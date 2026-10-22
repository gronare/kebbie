# Let tasks run without permission prompts

A task's Claude asks before it runs a tool its permission mode does not allow. On an isolated machine, such as a dev VM or a container you can restore, you can start tasks in Claude Code's `bypassPermissions` mode instead: Claude runs every tool without asking.

Claude Code recommends this mode only for an isolated container or VM. In it, Claude can change or delete anything your user can on that machine.

## Turn it on

1. Set it in `~/.config/kelpie/config.toml` for every repo:

   ```toml
   [task]
   permission_mode = "bypassPermissions"
   ```

   Or in a repo config for one repo, `~/.config/kelpie/repos/app.toml`, which wins.

2. Check the value in effect:

   ```bash
   kelpie config
   ```

3. Start a task. The first time on the machine, Claude shows its warning, `WARNING: Claude Code running in Bypass Permissions mode`, in the task's Space. Accepting it is yours: kelpie sends it no key. Answer it there, and kelpie types the task's prompt in once Claude's input box is empty, while the keeper or an open board runs.

For one task only, leave the config as it is and start it with the mode:

```bash
kelpie start search-filters --mode bypassPermissions "Add filters to search"
```

New tasks start in the mode. A task already running keeps the mode it started in, also when kelpie restarts it.

## What still holds

- A task never pushes nor approves its own commits. Its Claude settings deny `git push` and kelpie's user-only commands, and Claude Code applies deny rules in every mode. The worktree's own push block stops a push the rules miss.
- With `task.sandbox` on, the default, a task's shell commands still run in the sandbox: bypass mode skips the prompts, not the sandbox, and a command that fails in it is never retried outside it.
- You still push, or the lead does when you [let it](let-the-lead-push.md), and a [review](send-a-task-to-review.md) still holds the push until it passes.
- Managed settings with `permissions.disableBypassPermissionsMode = "disable"` keep Claude out of the mode. Claude then starts with permission prompts, and `kelpie start` says so.

## What you see

The task's row on the board says `bypass`, and the open task says `bypass permissions: Claude asks for no permission`. `kelpie status --json` gives `bypass_permissions: true` for it.

## Switch back

Remove `permission_mode` from the config, or set another mode such as `"acceptEdits"`. New tasks start without bypass. A running task keeps its mode. To change it, exit its Claude in the task's Space and run `kelpie start <name> --mode acceptEdits`: kelpie resumes the task's conversation in that mode.
