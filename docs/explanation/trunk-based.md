# Trunk-based by design

kelpie lands a task by rebasing its commits onto the default branch and pushing them there. It opens no pull request and makes no merge. This page explains why, and what takes the place of the pull request.

## Many small branches that do not live long

With several Claude sessions working at once, branches multiply fast. Every hour a branch stays open, the default branch moves under it, and the next task to land has more to reconcile. Pull requests make that worse: each one waits for a reviewer, and each wait lets the branches drift further apart.

kelpie keeps branches short instead. A task is meant to finish, land and free its worktree. `kelpie push` rebases it onto the remote's default branch first, so what you push is exactly what you read, on top of what everyone else has pushed. However far behind the task is, that rebase is part of the push. When the commits it goes over changed files the task changes too, kelpie hands the task to its Claude to run their tests, and pushes once it reports ready with the same commits. When the rebase conflicts, kelpie aborts it, leaves the worktree as it was, and hands the conflict to the task's Claude, which knows the change best.

A task that lives longer, a refactor under way while others land, can follow main instead of waiting for its push. Switch it on with `u` on the board or `kelpie follow <name> on`, and the keeper rebases it each time main moves, whenever its Claude sits idle with its work committed. Clean rebases happen quietly; when main's new commits touch the task's own files, its Claude runs their tests; a conflict goes to its Claude once, like any other. The task stays close to main the whole time, so its push holds no surprises. For a single catch-up, `e` on the board rebases now.

The result on the default branch is a straight line of commits, each written to stand on its own.

## What replaces the pull request

A pull request does three jobs: it shows you the change, it gets a second opinion, and it gates the merge. kelpie does each of them where you already are.

- **Seeing the change.** The review split shows the task's diff next to its Claude, and your comments go straight into Claude's input. The push window shows the sign-off card and every commit message before anything is pushed.
- **A second opinion.** The lead checks every ready task's commits against your rules skill. For risky work, the reviewer gate adds an independent, read-only Claude that must pass the current code. See [The review gate](review-gate.md).
- **The gate.** Tasks cannot push. You push, or the lead pushes a task you gave it permission for. See [The trust boundary](trust-boundary.md).

The gate is the push itself, so nothing waits on a web page.

## Commits matter more

Without a merge to group them, each commit on the default branch is read on its own. That is why kelpie leaves commit style to a skill (`task.rules_skill`) that the task follows, the lead checks, and `kelpie tidy` and the push window's edit and squash keys help you fix. The default, `kelpie:work-rules`, asks for one logical change per commit with a body that says why. Replace it with your own team's rules.

## CI after the push

Without a pull request, the default branch is where CI runs, so a task is finished when its commits are on the default branch and CI passes them, not before. When origin is on GitHub and `gh` is logged in, kelpie watches the workflow runs for the pushed commit and keeps the task open, with its worktree and its Claude, under CI running. Once every run is green it closes the task as done. When a run fails, the task moves to Needs you and its Claude gets the failing job's log, so the session that wrote the change fixes it while it still knows the change. The fix is a new commit and a normal push, and that push is watched in turn.

The board does the watching while it is open, every 30 seconds, and the watch is kept in the state directory, so it picks up again after a restart. Set `[ci] watch = false` for a repo whose CI says nothing useful, and its tasks close right after the push. See [`kelpie ci`](../reference/commands.md#kelpie-ci---json).

## Pushing in parts

A task does not have to land all at once. `kelpie push --through <sha>`, or `p` in the push window, pushes the commits up to one and keeps the task open, with the rest of its branch rebased on top. A long task can land its safe first steps while it keeps working. See [Push part of a task](../how-to/push-part-of-a-task.md).

## When you need a pull request anyway

Some repos require one. kelpie does not open it for you, but nothing stops you: push the task's branch yourself from the main checkout, for example `git -C ~/code/app push origin billing`, and open the pull request as usual. Once it has merged, `kelpie done` closes the task, because every change on the branch is on the default branch.
