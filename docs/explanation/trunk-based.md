# Trunk-based by design

kebbie lands a task by rebasing its commits onto the default branch and pushing them there. It opens no pull request and makes no merge. This page explains why, and what takes the place of the pull request.

## Many small branches that do not live long

With several Claude sessions working at once, branches multiply fast. Every hour a branch stays open, the default branch moves under it, and the next task to land has more to reconcile. Pull requests make that worse: each one waits for a reviewer, and each wait lets the branches drift further apart.

kebbie keeps branches short instead. A task is meant to finish, land and free its worktree. `kebbie push` rebases it onto the remote's default branch first, so what you push is exactly what you read, on top of what everyone else has pushed. However far behind the task is, that rebase is part of the push. When the commits it goes over changed files the task changes too, kebbie hands the task to its Claude to run their tests, and pushes once it reports ready with the same commits. When the rebase conflicts, kebbie aborts it, leaves the worktree as it was, and hands the conflict to the task's Claude, which knows the change best.

## One push line per repo

When several tasks of one repo touch the same files, every landing changes what the next one rebases onto. Rebasing them all ahead of time only multiplies that: each landing undoes the rebases done before it, and every task goes back to its Claude again. So each repo has one push line, kept by the keeper. A push you or the lead asks for joins the end of it, and the board says its place: `pushing for you · 2nd in line after Billing totals`. Only the head is fetched and rebased onto origin's default branch, never the local one, and only when its turn comes; tasks not in the line are left alone. When the head rebases cleanly and the overlap rules pass, kebbie pushes it and takes the next. When its rebase goes over the task's own files or conflicts, the head goes to its Claude and keeps its place while the line waits for it, since the pushes behind it would otherwise rebase onto a default branch that is about to move again. The wait has a ceiling: after 30 minutes without a ready report, or at once on a needs-you report, the head leaves the line for Needs you with the reason, and the next push goes on. Repos have lines of their own and never wait for each other.

A task that lives longer, a refactor under way while others land, can follow main instead of waiting for its push. Switch it on with `u` on the board or `kebbie follow <name> on`, and the keeper rebases it each time main moves, whenever its Claude sits idle with its work committed. Clean rebases happen quietly; when main's new commits touch the task's own files, its Claude runs their tests; a conflict goes to its Claude once, like any other. The task stays close to main the whole time, so its push holds no surprises. For a single catch-up, `e` on the board rebases now.

The result on the default branch is a straight line of commits, each written to stand on its own.

## What replaces the pull request

A pull request does three jobs: it shows you the change, it gets a second opinion, and it gates the merge. kebbie does each of them where you already are.

- **Seeing the change.** The review split shows the task's diff next to its Claude, and your comments go straight into Claude's input. The push window shows the sign-off card and every commit message before anything is pushed.
- **A second opinion.** A read-only reviewer checks every ready task's commits against your rules skill. For risky work, the reviewer gate adds an independent, read-only Claude that must pass the current code. See [The review gate](review-gate.md).
- **The gate.** Tasks cannot push. You push, or the lead pushes a task you gave it permission for. See [The trust boundary](trust-boundary.md).

The gate is the push itself, so nothing waits on a web page.

## Commits matter more

Without a merge to group them, each commit on the default branch is read on its own. That is why kebbie leaves commit style to a skill (`task.rules_skill`) that the task follows, the reviewer checks, and `kebbie tidy` and the push window's edit and squash keys help you fix. The default, `kebbie:work-rules`, asks for one logical change per commit with a body that says why. Replace it with your own team's rules.

## CI after the push

Without a pull request, the default branch is where CI runs, so a task is finished when its commits are on the default branch and CI passes them, not before. When origin is on GitHub and `gh` is logged in, kebbie watches the workflow runs for the pushed commit and keeps the task open, with its worktree and its Claude, under CI running. Once every run is green it closes the task as done. When a run fails, the task moves to Needs you and its Claude gets the failing job's log, so the session that wrote the change fixes it while it still knows the change. The fix is a new commit and a normal push, and that push is watched in turn.

The keeper does the watching, every 30 seconds (an open board does with `[keeper] enabled = false`), and the watch is kept in the state directory, so it picks up again after a restart. Set `[ci] watch = false` for a repo whose CI says nothing useful, and its tasks close right after the push. See [`kebbie ci`](../reference/commands.md#kebbie-ci---json).

## Trying work on local main

Some repos are run from their main checkout: kebbie itself when you run it from your own clone, or an app whose dev setup lives there. A task's worktree is the wrong place to try those, so a repo can opt in to staging with `worktree.stage_on_main`. `kebbie stage`, or `M` on the board, puts a ready task's commits on the main checkout's local default branch, unpushed, and several tasks stack there in the order you staged them. You try them together, as they will land, and push main yourself once it is right.

Staging stays inside the trunk-based model rather than beside it. The commits are cherry-picked, not merged, so main stays a straight line; a staged task never goes through the push line, since it goes out with main; and it stays open until its commits are on origin, so the keeper closes it like any other landed task. Two limits keep your own work safe: kebbie never rewrites a commit of yours on local main, so it refuses to stage or unstage while one sits there, and a stage that conflicts leaves main exactly as it was and hands the rebase to the task's Claude. See [Try a task before pushing](../how-to/try-a-task-before-pushing.md).

## Work that waits for other work

Sometimes one change has to land before another can start: an API field before the page that shows it. kebbie does not stack the second branch on the first. Stacked, the second task would carry the first one's commits through its own review and push, and every rewrite of the first (a review fix, a tidy, a rebase) would leave the second conflicting with code it never wrote. Instead the second task waits, and starts from the default branch once the first has landed there.

`kebbie start web-billing "<prompt>" --repo web --after billing@api` queues it. While it waits it has no worktree, Space or Claude, only its place under Queued on the board. kebbie counts the first task as finished when it has ended as done. In a repo where kebbie watches CI, a task closes as done only once CI is green for what it pushed, so the second task starts on a default branch CI has passed; if you close the first task as done while its CI still runs, kebbie keeps watching that CI for the waiting task and starts it on green. A task closed by `kebbie routine ok` counts as done too. Without CI, the first task's commits on the default branch are enough. A task that lands no commits, an investigation that ends with a report, is finished when it ends as done.

While the first task is still open, the second one waits quietly, whatever holds the first up: CI running or red, a review, a conflict its Claude is resolving. Those are the first task's business. When the first task is dropped or reset instead, is closed as done while its CI is red, or disappears without ending as done, the second one does not start and does not go away either: it moves to Needs you with the reason. Dropping the first often means the approach changed, not that the second is unwanted, so whether it starts anyway or goes is your call. See [`kebbie after`](../reference/commands.md#kebbie-after-name).

## Pushing in parts

A task does not have to land all at once. `kebbie push --through <sha>`, or `p` in the push window, pushes the commits up to one and keeps the task open, with the rest of its branch rebased on top. A long task can land its safe first steps while it keeps working. See [Push part of a task](../how-to/push-part-of-a-task.md).

## When you need a pull request anyway

Some repos require one. kebbie does not open it for you, but nothing stops you: push the task's branch yourself from the main checkout, for example `git -C ~/code/app push origin billing`, and open the pull request as usual. Once it has merged, `kebbie done` closes the task, because every change on the branch is on the default branch.
