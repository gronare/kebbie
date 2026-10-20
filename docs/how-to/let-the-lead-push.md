# Let the lead push

Let the lead push tasks without you, under a permission you can take back at any time.

## One task

Select the task on the board and press `a`. The lead may now push it once it is landed and its commits hold to your rules. Press `a` again to take it back. `done`, `drop` and `reset` take it back too.

A fix routine with `push = true` gives each of its runs' tasks this permission when the run starts. See [Routines](../explanation/routines.md#the-push-exception).

## Every reviewed task in a repo

1. Press `2` on the board for the Areas page.
2. Select the repo's area and press `a`.

The lead may now push any task in that repo once its reviewer has passed its current code. Send tasks there to review with `v`. Press `a` again to take it back.

The Areas page needs the vault.

## What still holds

- The lead's rules approval must match the commits it pushes.
- A ready task without a permission waits for you under Ready to push.
- After every push the lead is told which commits landed.
- Tasks never push, whatever the permission.

See also: [The trust boundary](../explanation/trust-boundary.md).
