# Decide what a task does with what it notices

Choose with `task.also_noticed` what a task does with things it notices outside its task: a bug nearby, a stale doc, a refactor worth doing.

| Value | The task |
| --- | --- |
| `"backlog"` (default) | Leaves them alone and files each one, written out as a backlog item: in the vault's backlog when the vault takes new items, else with `kebbie backlog add`. |
| `"fix"` | Fixes a small one beside the code it is changing, in a commit of its own, and files the rest. |
| `"ask"` | Puts each one to you with `kebbie ask` before it changes anything for it: fix it, file it or leave it. |

1. Set it in `~/.config/kebbie/config.toml` for every repo:

   ```toml
   [task]
   also_noticed = "fix"
   ```

2. Check the value in effect:

   ```bash
   kebbie config
   ```

New tasks follow it from their start.

If you want it for one repo, set it in that repo's config, `~/.config/kebbie/repos/app.toml`, which wins.

## What you see

The task's sign-off card lists each thing under **Also noticed**, with what the task did: where it filed it, the commit that fixed it, or what you answered. A line marked `not filed` is one the task could not file. The lead files it.

kebbie files each Also noticed line under Found by tasks at the top of the board's backlog, and the line on the card ends with its item, `(found:f3)`. A thing a second task notices counts on the same item, with both tasks as its sources, so one problem keeps one item however many tasks meet it. From the item you start a task, add it to a running task or dismiss it, with `o` or [`kebbie found`](../reference/commands.md#kebbie-found---json). Something in the task's own way is not listed here: it is an [obstacle](../explanation/obstacles-and-findings.md).

## The reviewer reads it too

Under `"backlog"` and `"ask"`, a change the task did not need is a finding. Under `"fix"`, a small fix in its own commit passes, and one mixed into the task's commits does not.
