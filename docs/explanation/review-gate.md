# The review gate

kelpie has two kinds of review, and they answer different questions. The review split is you reading the change. The reviewer gate is a second Claude reading it, with a verdict that holds the push until it passes.

## Two checks that are always separate

Every ready task gets the lead's rules check: the lead reads the task's commits against your rules skill and runs `kelpie approve`, or tells the task what to fix. That check is about how the work is shaped.

The reviewer is about whether the code is right. A task you send to review (`v` on the board, or `kelpie review`) waits under To review until a separate Claude has read it and given `verdict: ok` on the code its HEAD makes. Until then the board offers no push for it.

A task in review shows under Ready to push only with both: the lead's approval and the reviewer's pass.

## Why a separate Claude

The task's own Claude has written the change and believes in it. The lead has the whole picture but also a stake in moving work along. A reviewer that starts fresh, with no part in writing the code, reads it the way a colleague would.

So the reviewer is given what a colleague would want and nothing more: the [`kelpie:review`](../../skills/review/SKILL.md) skill as its instructions, the commits against the default branch, the task's prompt and later messages so it knows what was asked, your rules skill, and the repo's `task.also_noticed`. That setting draws the task's scope: under `"backlog"` and `"ask"` a change the task did not need is a finding, and under `"fix"` a small fix in a commit of its own is not. It runs with none of your own settings, hooks or plugins.

## Why read-only

A reviewer that can edit will fix what it finds, and then nobody has reviewed the fix. kelpie limits it to reading: Read, Grep, Glob and the shell for `git log`, `git diff` and `git show` alone, in `dontAsk` mode, with a hook that refuses every other command.

Its findings go to the lead, which passes the ones that matter back to the task. The task fixes them, reports ready again, and kelpie starts the reviewer on the changed code.

## What keeps a pass

A pass belongs to the code, not to the commits. A clean rebase, a reworded message or a squash keep the pass, because the code is the same. A changed line does not; the row shows `review stale` and the reviewer runs again when the task next reports ready.

This is what lets you tidy history in the push window without paying for a second review.

## Proof of the test run

A task's Claude runs the tests related to its change before it reports ready; the full suite is CI's job after the push. kelpie records each of those runs from the task's hooks, with its command, its result and the commit it ran on, so nobody has to run them again to know they passed.

The reviewer gets the latest runs in its brief, each marked as run on the code under review or on older code, and treats a failed run or no passing run on the reviewed code as a finding. The board's task row and the push window show the latest run: green when it passed on the code the branch holds now, yellow once a commit changed the code after it, red when it failed. Like a pass, a run belongs to the code, so a clean rebase or a squash keeps it current. See [`kelpie evidence`](../reference/commands.md#kelpie-evidence-name---json).

## When it is worth it

Most tasks go straight to push after the lead's check. The reviewer costs a full Claude run and some waiting, so send the tasks where a second reading pays: money, data, migrations, security, concurrency and wide refactors.

You can always take a task back out with `v` again or `kelpie review --withdraw`. Only you can withdraw: the gate is there for the times you asked for it, and neither the lead nor a task can talk its way past it.

The gate also carries push permission. On the Areas page, `a` lets the lead push every task in a repo once its reviewer has passed the current code. See [Let the lead push](../how-to/let-the-lead-push.md).
