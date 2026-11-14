# Work in subtasks

Have a task list its work as titled subtasks, each a slice of the work in one or more small commits, and push the accepted ones from the top of the branch while the task builds on.

Tasks work in subtasks by default. Turned off, a task commits its own way and you push it as a whole or in part (see [Push part of a task](push-part-of-a-task.md)).

## Let the task list its subtasks

1. The task's agent runs `kebbie subtasks` before its first line of code, with its subtasks in the order they go out, preparatory ones first:

   ```bash
   kebbie subtasks "Add the refunds table" "Refund an invoice" "Document refunds"
   ```

   Each subtask gets a short id from its title: `add-refunds`, `refund-invoice`, `document-refunds`.

2. It commits each subtask in small commits, each deployable on its own. Its commits go to the first subtask not done; when a subtask's work is finished, it marks it ready:

   ```bash
   kebbie subtasks done add-refunds
   ```

3. See them with `kebbie subtasks billing`:

   ```
   billing@app's subtasks:
     ● 1. Add the refunds table [add-refunds]  ready · 2 commits
          a1b2c3d Add the refunds table
          b2c3d4e Index refunds by invoice
     ◐ 2. Refund an invoice [refund-invoice]  in progress · 1 commit
          c3d4e5f Refund an invoice in full
     ○ 3. Document refunds [document-refunds]
   Pushable: nothing yet; the top commit's subtask is not accepted
   ```

   `○` is planned, `◐` in progress, `●` ready and waiting for you to accept it, `✓` accepted, and `↑ 2 pushed` counts the subtasks already on the default branch.

- If the work needs a subtask the task did not list, it adds one with its reason, `kebbie subtasks add "Backfill old refunds" --because "<why>"`, placed before the subtask it prepares with `--prepares <id>`. You see the reason beside the subtask.
- If the split is wrong, tell the task. It lists its subtasks again: a subtask keeps its id and its commits under the same title, and one left out that still holds commits stays, marked `(not in the list)`.
- If the task rebases, rewords or reorders its commits, each keeps its subtask, since kebbie places a commit by its change. A subtask's commits need not lie together in the branch.

## Accept and push the run

The push order is the branch's commits, oldest at the top. The **pushable run** is the commits from the top that you accepted, up to the first one you have not, never splitting a group. Accepting is how you push part of a task; pushing all of it needs no accept unless you [require every subtask accepted](#require-every-subtask-accepted).

1. Read a ready subtask in review, the whole subtask or one commit at a time, and accept it with `a`. From the command line:

   ```bash
   kebbie accept billing --subtask add-refunds
   ```

   Only you can accept a subtask, and only once it is ready. The acceptance belongs to each commit's code: a clean rebase or a new message keeps it, a changed line takes it back. `kebbie accept billing a1b2c3d` accepts one commit, and `--withdraw` takes either back.

2. Push the accepted run. In the push window, `◂ through here` marks the run's last commit, and `p` pushes the run and keeps the task open:

   ```
   Push?  y push all and close · p push the accepted run, 2 commits and keep it open · …
   ```

   The task keeps building: kebbie pushes the run without touching its worktree, rebased onto the default branch in a worktree of its own when the default branch moved, and tells its agent to rebase once its work in progress is committed. The pushed subtasks drop out of the list and show as a count. See [Push part of a task](push-part-of-a-task.md).

- If an accepted subtask has a commit behind one of a subtask you have not accepted, it shows `accepted · waits for <subtask>` and goes out once that one is accepted too.
- If an accepted subtask gains a commit, it is ready again, with only the new commit to accept, and its accepted commits stay in the run.

## Reorder the push order

The branch's order is the order commits go out. The task moves a preparatory commit it made late up to before the commit it prepares, and binds commits that only work together into a group, both through kebbie, which replays the branch and keeps each commit's subtask and acceptance:

```bash
kebbie order move a1b2c3d --before 9f8e7d6 --because "the index needs the frontmatter first"
kebbie order group 4d5e6f7 8a9b0c1 --because "the migration and its model go out together"
```

You or the lead can run the same for any task. A move is refused while a push of the task waits in line. When a commit does not apply in its new place, the branch stays as it was. See [`kebbie order`](../reference/commands.md#kebbie-order-name).

## Test each commit on its own

Once a task reports ready, the keeper tests each of its unpushed commits on its own, one at a time, in a throwaway worktree of kebbie's: with the test commands the task's own recorded runs used, else `task.test_commands`. The push window, `kebbie order` and the phone mark each commit `tests ✓`, `tests ✗`, `testing…` or `not tested`. Nothing waits on the result.

- A commit is tested once: the result is kept by its sha, and a commit the task's own recorded run already tested is not run again.
- A task in the sandbox gets only what its own recorded runs say, since kebbie runs none of its code outside the sandbox.
- It costs a suite run per commit. In a repo whose suite takes minutes, turn it off with `[tests] per_commit = false` in the repo's config. See [`tests.per_commit`](../reference/configuration.md#ci-updates-and-the-keeper).

## Require every subtask accepted

Turn on `task.require_accept` when no commit should leave without you accepting it first.

1. Set it in the machine config for every repo, or in one repo's own file for that repo alone:

   ```toml
   # ~/.config/kebbie/config.toml, or ~/.config/kebbie/repos/app.toml
   [task]
   require_accept = true
   ```

   Or press space on `task.require_accept` on the board's [Settings page](../reference/board.md#settings-page).

2. Accept each subtask once it is ready. A ready task with a subtask to accept waits under Needs you, with `accept subtask 2 · r review` under its row, and `N` lists it under Stopped on you instead of Push, in this order.

3. Push as before. A push that would take a commit you did not accept is refused, and says which:

   ```
   Not pushed: subtask 2 is not accepted
   Accept it with kebbie accept billing --subtask refund-invoice, or with a in its review (r on the board); task.require_accept holds every commit until it is accepted.
   ```

   `p` still pushes the accepted run. In push in order, the first task refused stops the rest, which stay ready.

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

- [`kebbie subtasks`](../reference/commands.md#kebbie-subtasks-name-subtask-) and [`kebbie accept`](../reference/commands.md#kebbie-accept-name-sha)
- [Push part of a task](push-part-of-a-task.md)
- [Review a task's changes](review-a-task.md)
