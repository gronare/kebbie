# Let kebbie and the lead push

Let kebbie push your ready tasks without you, and the lead too, under a permission you can take back at any time.

## One task

1. Select the task on the board.
2. Press `a`.

kebbie now pushes the task itself once it reports ready and its commits pass the rules check of the read-only reviewer that checks them, and through any review the task is in.

- If you want the permission back, press `a` again. `done`, `drop` and `reset` take it back too.
- If a fix routine has `push = true`, each of its runs' tasks gets this permission when the run starts. See [Routines](../explanation/routines.md#the-push-exception).

## Every ready task in a repo

1. Press `3` on the board for the Areas page.
2. Select the repo's area.
3. Press `a`.

kebbie now pushes any ready task in that repo itself, reviewed or not: a review is optional. A task you send to review with `v` waits for its pass first.

- If you want the permission back, press `a` on the area again.
- If you run without the vault, there is no Areas page: it needs the vault.

## What still holds

- The reviewer's passed rules check, or the lead's `kebbie approve`, must match the commits kebbie pushes; the lead may still push such a task itself, and reads the diff before it does.
- kebbie pushes a task under a permission itself as soon as it reports ready and a passed rules check matches its commits: it joins its repo's push line, which rebases and pushes it with no test run, holds it behind the other half its ready report names, and watches CI. The lead hears what went out and need not be awake.
- A ready task without a permission waits for you under Ready to push. One under a permission stands under Going out, marked `kebbie pushes it under your permission`, with no read count, subtask to accept or yellow under its row: accepting is the lead's and pushing kebbie's there, unless [`task.require_accept`](../reference/configuration.md) is on, which leaves accepting to you.
- After every push the lead is told which commits landed.
- The same permission lets the lead [stage](try-a-task-before-pushing.md) a task on local main in a repo that turns on `worktree.stage_on_main`.
- Tasks never push, whatever the permission.

See also: [The trust boundary](../explanation/trust-boundary.md).
