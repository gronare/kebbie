# Push to a branch per task

Push each task of a repo to a branch of its own, for a repo whose default branch is for releases only: a push to it builds and ships the app, and work goes to feature branches that are merged when it is time to release.

## Turn it on

1. Open the repo's config, `~/.config/kebbie/repos/<repo>.toml`.
2. Add the branch, with `{task}` where the task's name goes:

   ```toml
   [push]
   branch = "feature/{task}"
   ```

3. Check it with `kebbie config` and `kebbie doctor` in the repo: both name the branch.

Tasks already on the board follow it from their next push.

## What a push does then

1. You push a ready task as before, from the board, the phone or `kebbie push`.
2. kebbie pushes its commits to `feature/<task>` on origin, as they are: nothing is rebased onto the default branch, which stays as it was.
3. The task moves under **On branch, waiting for main**, its row naming the branch.
4. You, or your release flow, merge the branch into the default branch when it is time.
5. kebbie closes the task as done once its commits are on the default branch, and tells the lead.

- More work on the task goes in new commits on top. Its next push adds them to the same branch by fast-forward.
- kebbie never force-pushes. If the pushed commits were rewritten, the push is refused until the task's Claude puts its new commits on top of `origin/feature/<task>`.
- The rules check, the review and the test runs a push needs hold as before.
- kebbie watches no CI for the branch, so a TestFlight build or another branch build is yours to follow.

## Limits

- Each task waiting on its branch keeps its worktree until it closes. `d` closes it only once its commits are on the default branch; drop it to free the worktree sooner, and its commits stay on its branch on origin.
- kebbie opens no pull request and merges nothing.
- See [`push.branch`](../reference/configuration.md#push) for the key itself.
