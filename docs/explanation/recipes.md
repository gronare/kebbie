# Recipes

This page explains why the lead names a recipe for every task it starts, and where the choice is kept.

A recipe is the shape of a task: what it does first and where it stops for you. The lead picks one from its playbook, the skill [`kebbie:playbook`](../../skills/playbook/SKILL.md), before it starts a task.

## Nine shapes cover the work

| Recipe | When |
| --- | --- |
| `small-fix` | The prompt settles the change: one step, the fix with the test that goes red without it, no mockup and no plan. |
| `feature-with-ui` | A screen to design: a mockup until you say yes, then the task's own plan, then the build in small steps. |
| `task-after-task` | It builds on another task's change, so it is queued until that one is on main. |
| `rollout-in-stages` | It must reach production in stages, such as an expand and contract rename: one task, a push per stage. |
| `follow-up` | A comment on a pushed commit, or red CI after a push. |
| `research` | A question to settle before anything is built: a task of its own that ends with your choice. |
| `across-repos` | One change in several repos, as one task with a half per repo or tasks that wait for each other. |
| `waiting-on-outside` | The next step waits on a deploy, a release, a date or a person, so the task is parked or starts later. |
| `same-files` | It would change files a running task changes, so it waits for that task or joins it. |

Your yes is the gate on what is built, and your push is the gate on what goes out. A recipe never adds a gate of its own.

## Naming it is required, so it is never skipped

A choice the lead makes in its head leaves nothing to look at when the task goes the wrong way. So [`kebbie start`](../reference/commands.md#kebbie-recipe-task-recipe-why) refuses the lead a task without `--recipe "<recipe>: <why>"`, and lists the recipes when it does. A reminder in the lead's role can be overlooked on a busy turn; a refused start cannot.

You start a task by hand without one, and it shows no recipe.

## The board shows the shape, the note keeps the why

The board shows `recipe: small fix` under the task's row, beside the [assessor's](review-gate.md#an-assessor-judges-whether-the-work-needs-a-review) word on its review, and the open task adds the lead's why. The board shows only how things stand now.

The why is history, and history lives once, in the task's note: the vault note when the task has one, else kebbie's own `notes/<task>@<repo>.md` in its state. kebbie writes the recipe and the assessor's word in its own note; a task with a vault note is told to write them there, and writes its own decisions beside them as it goes: the mockup you approved, its plan's steps and why they changed. A Codex session has no vault tools, so for a Codex task the lead writes them.

## A correction has something to correct

When you tell the lead a task was done the wrong way, its recipe and why are on record: the signal it saw, and what it made of it. That is what the lead learns from. It does not rewrite its own rules; you change the playbook when the shapes themselves are wrong.
