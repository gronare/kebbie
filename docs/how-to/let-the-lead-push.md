# Let the lead push

Let the lead push tasks without you, under a permission you can take back at any time.

## One task

Select the task on the board and press `a`. The lead may now push it once its commits are rules-approved, by the read-only reviewer that checks them when the task reports ready, and through any review it is in. Press `a` again to take it back. `done`, `drop` and `reset` take it back too.

A fix routine with `push = true` gives each of its runs' tasks this permission when the run starts. See [Routines](../explanation/routines.md#the-push-exception).

## Every reviewed task in a repo

1. Press `2` on the board for the Areas page.
2. Select the repo's area and press `a`.

The lead may now push any task in that repo once its reviewer has passed its current code. Send tasks there to review with `v`. Press `a` again to take it back.

The Areas page needs the vault.

## What still holds

- A rules approval, the reviewer's or the lead's, must match the commits it pushes, and the lead reads the diff itself before its push.
- A ready task without a permission waits for you under Ready to push.
- After every push the lead is told which commits landed.
- The same permission lets the lead [stage](try-a-task-before-pushing.md) a task on local main in a repo that turns on `worktree.stage_on_main`.
- Tasks never push, whatever the permission.

See also: [The trust boundary](../explanation/trust-boundary.md).
