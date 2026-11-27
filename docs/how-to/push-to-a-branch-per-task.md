# Push where main is for releases only

Push a repo's tasks somewhere other than the default branch, for a repo whose default branch is for releases only: a push to it builds and ships the app. Each task can get a feature branch of its own, or the tasks can share the next release branch, which kebbie creates with your version bump when it is missing.

## A branch per task

1. Open the repo's config, `~/.config/kebbie/repos/<repo>.toml`.
2. Add the branch, with `{task}` where the task's name goes:

   ```toml
   [push]
   branch = "feature/{task}"
   ```

3. Check it with `kebbie config` and `kebbie doctor` in the repo: both name the branch.

A push then goes to `feature/<task>` on origin as it is: nothing is rebased onto the default branch, which stays as it was. More work on the task goes in new commits on top, and its next push adds them to the same branch by fast-forward. kebbie never force-pushes: if the pushed commits were rewritten, the push is refused until the task's Claude puts its new commits on top of `origin/feature/<task>`.

## The next release branch

1. In the repo's config, name the release branch with `{next}`, and the step that prepares a new one:

   ```toml
   [push]
   branch = "release/{next}"
   choices = ["feature/{task}", "release/{current}"]
   prepare = "scripts/bump-version"

   [push.version]
   file = "App.xcodeproj/project.pbxproj"
   key = "MARKETING_VERSION"
   ```

2. Make the prepare step set the version from `KEBBIE_VERSION`, which kebbie sets to the version in the branch's name, such as `3.5`. For an Xcode project it can be `sed -i '' "s/MARKETING_VERSION = .*;/MARKETING_VERSION = $KEBBIE_VERSION;/" App.xcodeproj/project.pbxproj`.
3. Check it with `kebbie config` and `kebbie doctor` in the repo.

Without `[push.version]`, `{next}` counts from the release branches on origin: the open one, not yet merged into the default branch, else the highest plus one. With `file`, it is that file's version on the default branch plus one: `3.4.3` goes to `3.5`. `bump = "patch"` makes it `3.4.4`. With `command`, its output is `{next}`, so `branch = "{next}"` makes the command's output the branch.

When you push the first task:

1. Its row under Ready to push reads `ready to push to release/3.5`, and the push window names `release/3.5 · new: kebbie creates it from main, push.prepare first` under `TO`.
2. kebbie creates `release/3.5` from the default branch, runs the prepare step there, commits it as `Prepare release/3.5` and pushes the branch.
3. It rebases the task's commits onto the branch, in a worktree of its own, and pushes them on top.

The next task goes onto the same branch, after the first, for as long as it has not reached the default branch. Its own branch in its worktree stays as it was.

## Pick another branch for one push

In the push window, `t` moves under `TO` to the next branch the repo offers: those in `choices`, such as `feature/{task}` for a TestFlight build or `release/{current}`, and last the default branch. A push goes to the default branch only when you pick it there, or run `kebbie push --to {default}`; the lead's and kebbie's pushes always follow `branch`.

## After the push

1. The task moves under **On branch, waiting for main**, its row naming the branch.
2. You, or your release flow, merge the branch into the default branch when it is time.
3. kebbie closes the task as done once its commits are on the default branch, and tells the lead.

- The rules check, the review and the test runs a ready report needs hold as before.
- kebbie watches no CI for the branch, so a TestFlight build or another branch build is yours to follow.

## Limits

- Each task waiting on its branch keeps its worktree until it closes. `d` closes it only once its commits are on the default branch; drop it to free the worktree sooner, and its commits stay on the branch on origin.
- A task whose commits conflict with what is on a shared branch is refused with the files; settle it on the branch yourself, or push it once the branch has reached the default branch.
- `{next}` counts a release branch as merged once its last commit is on the default branch. A release branch squash-merged into it, or deleted from origin before kebbie fetched again, still counts as open; set `push.version.file` to count from the released version instead.
- kebbie opens no pull request and merges nothing.
- See [`push.branch`](../reference/configuration.md#push) for the keys themselves.
