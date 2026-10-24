# Decide what a task does with what it notices

A task's Claude notices things outside its task: a bug nearby, a stale doc, a refactor worth doing. `task.also_noticed` decides what it does with them.

| Value | The task |
| --- | --- |
| `"backlog"` (default) | Leaves them alone and files each one: in the vault's backlog with the vault plugin, else in the local backlog with `kebbie backlog add`. |
| `"fix"` | Fixes a small one beside the code it is changing, in a commit of its own, and files the rest. |
| `"ask"` | Puts each one to you with `kebbie ask` before it changes anything for it: fix it, file it or leave it. |

1. Set it in `~/.config/kebbie/config.toml` for every repo:

   ```toml
   [task]
   also_noticed = "fix"
   ```

   Or in a repo config for one repo, `~/.config/kebbie/repos/app.toml`, which wins.

2. Check the value in effect:

   ```bash
   kebbie config
   ```

New tasks follow it from their start.

## What you see

The task's sign-off card lists each thing under **Also noticed**, with what the task did: where it filed it, the commit that fixed it, or what you answered. A line marked `not filed` is one the task could not file; the lead files it.

The reviewer reads the setting too. Under `"backlog"` and `"ask"` a change the task did not need is a finding; under `"fix"` a small fix in its own commit passes, and one mixed into the task's commits does not.
