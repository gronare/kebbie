# Work in steps

Have a task list its work as steps, one commit each, accept the commits one at a time, and push the accepted ones while the task builds the next.

It is off unless you turn it on. Without it, a task commits its own way and you push it as a whole or in part (see [Push part of a task](push-part-of-a-task.md)).

## Turn it on

1. Set `task.steps` in the machine config for every repo, or in one repo's config:

   ```toml
   # ~/.config/kebbie/config.toml
   [task]
   steps = true
   ```

   Or press space on `task.steps` on the board's Settings page.

2. Start a task. New tasks follow the setting; a running task follows it once it restarts.

## Let the task list its steps

1. The task's Claude runs `kebbie steps` before its first line of code, with its steps in the order they go out, preparatory ones first:

   ```bash
   kebbie steps "Add the refunds table" "Refund an invoice" "Document refunds"
   ```

2. See them with `kebbie steps billing`:

   ```
   billing@app's steps:
     ● 1. Add the refunds table  a1b2c3d
     ◐ 2. Refund an invoice
     ○ 3. Document refunds
   ```

   `○` is planned, `◐` in progress, `●` committed and waiting for you to accept it, `✓` accepted, and `↑ 2 pushed` counts the steps already on the default branch.

- If the split is wrong, tell the task. It lists its steps again, and every step not pushed yet can change.
- If a commit is not in the list, it shows as a step of its own, titled by its subject.

## Accept and push

1. Read a step's commit in review and accept it. From the command line:

   ```bash
   kebbie accept billing a1b2c3d
   ```

   Only you can accept a commit. The acceptance belongs to the commit's code: a clean rebase or a new message keeps it, a changed line takes it back. `kebbie accept billing a1b2c3d --withdraw` takes it back too.

2. Push the accepted steps. The push window starts at the last accepted commit, or run:

   ```bash
   kebbie push billing --through a1b2c3d
   ```

   The task stays open and keeps building the next step: kebbie pushes the accepted commits without touching its worktree, rebased onto the default branch in a worktree of its own when the default branch moved, and tells its Claude to rebase onto the default branch once its work in progress is committed. The pushed steps drop out of the list and show as a count. See [Push part of a task](push-part-of-a-task.md).

## Related

- [`kebbie steps`](../reference/commands.md#kebbie-steps-name-step-) and [`kebbie accept`](../reference/commands.md#kebbie-accept-name-sha)
- [Push part of a task](push-part-of-a-task.md)
- [Review a task's changes](review-a-task.md)
