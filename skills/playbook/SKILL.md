---
name: playbook
description: How a task gets done, from its start to its push: the lead's recipes and the task's procedures on the way. Use when you plan or start a task and name its recipe, when a task's work turns out another shape than its recipe, when work meets an obstacle on its own path, when a task's plan changes, and when a task writes its sign-off card.
---

# Playbook

A **recipe** is the shape of a task: its phases and where the user is needed. You pick one before you start a task, kebbie shows it under the task's row and writes it to the task's note, and the user reads it there when they ask why a task went the way it did. Your recipe and its why are the record a correction from the user is measured against, so make the why the signal you saw.

Every recipe ends at the user's push, or yours under a push permission they gave. Accepting each subtask before it goes is the user's option, never a gate a recipe adds.

## Steps

1. **Match.** Read the work against the recipes' **When** below. A recipe about how this task meets other work (task after task, follow-up, across repos, waiting on outside, same files) wins over one about the change's own shape (small fix, feature with UI, rollout in stages, research), and borrows that shape's phases: a feature with UI queued after another task still starts with its mockup. Done when one recipe fits and you can say in one line which of its signals holds.
2. **Name it.** Start the task with `--recipe "<recipe>: <why>"`, the why in the user's words: `--recipe "small-fix: the VAT rounding in one function, with its test"`. Write the prompt for the recipe's phases: what the task does first and where it stops for the user. Done when the start carries the recipe and the prompt names the first phase.
3. **Follow it.** When the work turns out another shape (a small fix that needs a screen, a feature that must land in stages), run `kebbie recipe <task> "<recipe>: <why>"` with what changed: kebbie tells the task its new recipe, and you tell it what that changes in its phases. Done when the recipe on the board is the shape the task is in.

## On the way

Three pages hold what happens between the start and the push. Read the one your moment calls for:

- [OBSTACLE.md](OBSTACLE.md): when a task meets an **obstacle** on its own path, or a review finding questions scope or intent. The task's side, and how kebbie and the lead carry out the investigator's four decisions.
- [PLAN.md](PLAN.md): when a task's plan changes: the loop of plan, build, finding and revision, and when a revision is the user's question.
- [CARD.md](CARD.md): when a task writes its sign-off card.

## Review by assessment

Whether a task needs a reviewer is the assessor's judgement of its work, as your role's Review station says: no recipe adds a review or skips one. Bring the user each `review suggested` with your recommendation, and send it on their word.

## The note holds the history

The task's note is where the why lives: kebbie writes the recipe and the assessor's word there, and a task with a vault note writes, as each phase ends, the approved mockup's link, its plan's subtasks and each change to them with the reason. Brief the task to keep it, so the board can stay the current state alone.

## The recipes

### `small-fix`

- **When**: the prompt settles what changes and how it behaves; one behaviour in a few files, no screen to design, nothing for the user to choose.
- **Phases**: one subtask: the fix and the test that goes red without it, in one commit or a preparatory one first. No mockup and no plan beyond that subtask.
- **The user**: the push.

### `feature-with-ui`

- **When**: a new or rebuilt screen, or behaviour the user will judge by seeing it.
- **Phases**:
  1. **Mockup.** The task publishes a mockup and puts it to the user as a plan holding its link, with `kebbie ask --plan`. Their yes is the only gate on what is built: it revises the mockup until they say yes.
  2. **Plan.** The task researches the code and lists its subtasks with `kebbie subtasks`. The plan is the task's own; it goes on without the user's approval.
  3. **Build.** Subtasks in order, each a slice of the work in small deployable commits, preparatory ones first, trunk-based; `kebbie subtasks done` hands each finished subtask to review. When the code contradicts the plan, the task revises it by [PLAN.md](PLAN.md): it adds a subtask with `kebbie subtasks add --because` or lists them anew, tells the lead why, and writes the reason in its vault note when it has one.
- **The user**: the mockup's yes, then the push.

### `task-after-task`

- **When**: this task builds on another's change: a field, an interface or a rename that task lands.
- **Phases**: queue it with `--after <task> --why "<what it builds on>"`. It starts from main once that task's commits are there, then runs its own shape.
- **The user**: nothing for the order.

### `rollout-in-stages`

- **When**: the change reaches production in steps with something live between them: a table or column renamed or dropped (expand, then contract), a backfill before a constraint, a migration the next commit needs deployed first, a flag turned on before the old code goes.
- **Phases**: one task. It lists its subtasks, then declares the stages in push order with `kebbie stages "<title>:<requirement>" …`, each requirement one of `deploy` (the stage's deploy is green), `check:<what the task checks>` (the task runs it once the stage is deployed) or `verified:<what the user verifies>` (a question to the user once it is deployed), and puts each subtask in its stage with `kebbie stages assign <stage> <subtask> …`. Each stage's commits sit above the next stage's. kebbie stops the pushable run at the first stage that is not met, follows CI and the deploy after each stage's push, and opens the next stage when the requirement holds; the task builds on meanwhile. A failed stage holds the rest: the task fixes it in a subtask of that stage (`kebbie subtasks add … --stage <stage>`) or raises it with `kebbie obstacle`.
- **The user**: a push per stage, and the answer on each verification.

### `follow-up`

- **When**: a colleague's comment on a pushed commit holds, or CI went red on a pushed task.
- **Phases**: the fix as a small fix: one commit with its test, in the task kebbie hands the red CI to, or the follow-up the comment's card starts.
- **The user**: the push, and for a comment, the reply to send.

### `research`

- **When**: a question to settle before anything is built: which library, why something behaves as it does, what a change would cost. The result is a decision, not code.
- **Phases**: a task of its own with its own agent, so your conversation with the user goes on. It reads, tries in probes, and puts what it found to the user as options with `kebbie ask`. It ends with the user's choice, written as a decision with its reasons, in the vault as a decision note when the vault is in use.
- **The user**: the choice. Nothing is pushed or reviewed.

### `across-repos`

- **When**: one change needs commits in more than one repo, such as a payload one repo sends and another consumes.
- **Phases**: one task with a half per repo (`--repo api --repo web`) when the halves land together. When the second needs the first on main, a task per repo, the second queued `--after <task>@<repo>`; when it needs a release of the first, the second waits as waiting on outside does.
- **The user**: a push per repo.

### `waiting-on-outside`

- **When**: the next step waits on something kebbie does not see: a deploy going green, CI in another system, a release, a date, a person.
- **Phases**: a new task starts at its time with `--at <time>`; a running one is parked until the condition should hold, on the user's word (`kebbie park <task> --until <when>`), with the condition in your summary. When it comes back, check the condition before it goes on.
- **The user**: the park.

### `same-files`

- **When**: the new work would change files a running task changes (its changed files in `kebbie status --json`, or its subtasks).
- **Phases**: queue it `--after <running task> --why "both change <file>"`, or, when it is the same change, fold it into the running task with `kebbie tell`.
- **The user**: nothing for the order.
