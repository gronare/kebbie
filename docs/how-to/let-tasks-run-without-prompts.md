# Let tasks run without permission prompts

Start tasks in Claude Code's `bypassPermissions` mode on an isolated machine, so Claude runs every tool without asking.

A task's Claude asks before it runs a tool its permission mode does not allow. On an isolated machine, such as a dev VM or a container you can restore, bypass mode skips those prompts.

> **Warning:** Claude Code recommends this mode only for an isolated container or VM. In it, Claude can change or delete anything your user can on that machine.

## Turn it on

1. Set it in `~/.config/kebbie/config.toml` for every repo:

   ```toml
   [task]
   permission_mode = "bypassPermissions"
   ```

2. Check the value in effect:

   ```bash
   kebbie config
   ```

3. Start a task. The first time on the machine, Claude shows its warning, `WARNING: Claude Code running in Bypass Permissions mode`, in the task's Space.

4. Answer the warning there. Accepting it is yours: kebbie sends it no key. kebbie types the task's prompt in once Claude's input box is empty, while the keeper or an open board runs.

5. The task's row on the board says `bypass`.

If you want it for one repo, set it in that repo's config, `~/.config/kebbie/repos/app.toml`, which wins.

If you want it for one task only, leave the config as it is and start the task with the mode:

```bash
kebbie start search-filters --mode bypassPermissions "Add filters to search"
```

New tasks start in the mode. A task already running keeps the mode it started in, also when kebbie restarts it.

## What still holds

- A task never pushes nor approves its own commits. Its Claude settings deny `git push` and kebbie's user-only commands, and Claude Code applies deny rules in every mode. The worktree's own push block stops a push the rules miss.
- With `task.sandbox` on, the default, a task's shell commands still run in the sandbox. Bypass mode skips the prompts, not the sandbox, and a command that fails in it is never retried outside it.
- You still push, or the lead does when you [let it](let-the-lead-push.md), and a [review](send-a-task-to-review.md) still holds the push until it passes.
- Managed settings with `permissions.disableBypassPermissionsMode = "disable"` keep Claude out of the mode. Claude then starts with permission prompts, and `kebbie start` says so.

## What you see

The task's row on the board says `bypass`, and the open task says `bypass permissions: Claude asks for no permission`. `kebbie status --json` gives `bypass_permissions: true` for it.

## Switch back

1. Remove `permission_mode` from the config, or set another mode such as `"acceptEdits"`. New tasks start without bypass.

2. A running task keeps its mode. To change it, exit its Claude in the task's Space and run:

   ```bash
   kebbie start <name> --mode acceptEdits
   ```

   kebbie resumes the task's conversation in that mode.
