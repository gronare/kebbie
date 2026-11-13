# Work in steps

Have a task list its work as steps, each a slice of the work in one or more small commits, and push them while the task builds the next: one at a time or together, accepting each first if you want to.

Tasks work in steps by default. Turned off, a task commits its own way and you push it as a whole or in part (see [Push part of a task](push-part-of-a-task.md)).

## Let the task list its steps

1. The task's Claude runs `kebbie steps` before its first line of code, with its steps in the order they go out, preparatory ones first:

   ```bash
   kebbie steps "Add the refunds table" "Refund an invoice" "Document refunds"
   ```

2. It commits each step in small commits, each deployable on its own. When a step's work is finished, it marks the step ready, and its next commits go to the next step:

   ```bash
   kebbie steps done
   ```

3. See them with `kebbie steps billing`:

   ```
   billing@app's steps:
     ● 1. Add the refunds table  ready · 2 commits
     ◐ 2. Refund an invoice  in progress · 1 commit
     ○ 3. Document refunds
   ```

   `○` is planned, `◐` in progress with the commits it holds so far, `●` ready and waiting for you to accept it, `✓` accepted, and `↑ 2 pushed` counts the steps already on the default branch.

- If the split is wrong, tell the task. It lists its steps again: every step not pushed yet can change, and each commit stays in the step it is in.
- If the task rebases, rewords or squashes its commits, they stay in their steps; a fixup folded into a commit stays in that commit's step.

## Accept and push

Accepting a step marks it as read and lets the push window start there. It is up to you: a step you did not accept pushes all the same, unless you [require every step accepted](#require-every-step-accepted).

1. Read a ready step in review, the whole step or one commit at a time, and accept it. From the command line:

   ```bash
   kebbie accept billing --step 1
   ```

   Only you can accept a step. The acceptance belongs to each commit's code: a clean rebase or a new message keeps it, a changed line takes it back. `kebbie accept billing a1b2c3d` accepts one commit, and `--withdraw` takes either back.

2. Push the accepted steps. In the push window, `p` on a commit pushes through the end of its step, or run `kebbie push` through the step's last commit:

   ```bash
   kebbie push billing --through a1b2c3d
   ```

   The task stays open and keeps building the next step: kebbie pushes the accepted commits without touching its worktree, rebased onto the default branch in a worktree of its own when the default branch moved, and tells its Claude to rebase onto the default branch once its work in progress is committed. The pushed steps drop out of the list and show as a count. See [Push part of a task](push-part-of-a-task.md).

## Require every step accepted

Turn on `task.require_accept` when no commit should leave without you accepting it first.

1. Set it in the machine config for every repo, or in one repo's own file for that repo alone:

   ```toml
   # ~/.config/kebbie/config.toml, or ~/.config/kebbie/repos/app.toml
   [task]
   require_accept = true
   ```

   Or press space on `task.require_accept` on the board's [Settings page](../reference/board.md#settings-page).

2. Accept each step once it is ready. A ready task with a step to accept waits under Needs you, with `accept step 2 · r review` under its row, and `N` lists it under Stopped on you instead of Push, in this order.

3. Push as before. A push that would take a commit you did not accept is refused, and says which:

   ```
   Not pushed: step 2 is not accepted
   Accept it with kebbie accept billing d4e5f6a, or with a in its review (r on the board); task.require_accept holds every commit until it is accepted.
   ```

   `kebbie push billing --through a1b2c3d` still pushes the accepted steps up to that commit. In push in order, the first task refused stops the rest, which stay ready.

- With `task.steps` off, `task.require_accept` has no effect.

## Turn it off

1. Set `task.steps = false` in the machine config for every repo, or in one repo's own file for that repo alone:

   ```toml
   # ~/.config/kebbie/config.toml, or ~/.config/kebbie/repos/app.toml
   [task]
   steps = false
   ```

   Or press space on `task.steps` on the board's [Settings page](../reference/board.md#settings-page); `tab` switches between This machine and the repo.

2. Start a task. New tasks follow the setting; a running task follows it once it restarts. `kebbie config` shows `task.steps = false` and the layer it comes from.

- If the machine has it off, `steps = true` in a repo's own file turns it on for that repo.

## Related

- [`kebbie steps`](../reference/commands.md#kebbie-steps-name-step-) and [`kebbie accept`](../reference/commands.md#kebbie-accept-name-sha)
- [Push part of a task](push-part-of-a-task.md)
- [Review a task's changes](review-a-task.md)
