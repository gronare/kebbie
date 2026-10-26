# Let the lead push

Let the lead push tasks without you, under a permission you can take back at any time.

## One task

1. Select the task on the board.
2. Press `a`.

The lead may now push the task once its commits are rules-approved, by the read-only reviewer that checks them when the task reports ready, and through any review the task is in.

- If you want the permission back, press `a` again. `done`, `drop` and `reset` take it back too.
- If a fix routine has `push = true`, each of its runs' tasks gets this permission when the run starts. See [Routines](../explanation/routines.md#the-push-exception).

## Every reviewed task in a repo

1. Press `2` on the board for the Areas page.
2. Select the repo's area.
3. Press `a`.
4. Send tasks in that repo to review with `v`.

The lead may now push any task in that repo once its reviewer has passed its current code.

- If you want the permission back, press `a` on the area again.
- If you run without the vault, there is no Areas page: it needs the vault.

## What still holds

- A rules approval, the reviewer's or the lead's, must match the commits it pushes, and the lead reads the diff itself before its push.
- A ready task without a permission waits for you under Ready to push.
- After every push the lead is told which commits landed.
- The same permission lets the lead [stage](try-a-task-before-pushing.md) a task on local main in a repo that turns on `worktree.stage_on_main`.
- Tasks never push, whatever the permission.

See also: [The trust boundary](../explanation/trust-boundary.md).
