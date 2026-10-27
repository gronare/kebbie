# Push part of a task

Land a task's first commits while the rest of its branch keeps working.

## From the board

1. Select the task.
2. Press `p` to open the push window.
3. Move to the last commit you want to land with `↑` `↓`.
4. Press `p`.

kebbie pushes the commits up to and including the selected one. The task stays open, and its Claude keeps working: kebbie never touches its worktree, its HEAD or its uncommitted changes.

- When the task's branch already sits on the default branch, the commits go out as they are, and the rest of the branch is on top of them.
- When the default branch moved on, kebbie rebases the commits onto it in a worktree of its own and pushes them from there. It then tells the task's Claude, which rebases its branch onto the default branch once its work in progress is committed; git drops the pushed commits as already there.
- When the default branch changed files those commits change too, or they conflict, kebbie asks the task's Claude to do that rebase first, run the tests (and resolve the conflict) and report ready, and pushes them then.

- If you accepted commits with [`kebbie accept`](../reference/commands.md#kebbie-accept-name-sha), they are marked `✓` and the window opens on the last of the accepted ones from the first commit, marked `◂ through here`. `p` there pushes them, and the task keeps working. This works while its Claude is still working too: the window then offers only the accepted ones.
- If the task works in steps, each commit shows its step's number, and the steps not committed yet show under the commits as what stays. See [Work in steps](work-in-steps.md).

## From the command line

1. Push through the last commit you want to land:

   ```bash
   kebbie push billing --through 3f2a91c
   ```


## Before you push

In the push window:

| Key | What it does |
| --- | --- |
| `e` | Edits the selected commit's message. |
| `s` | Squashes all commits into one. |

Only messages change; the code is identical. A commit that arrives after the window opened stops a push of all of them; a push of the first commits goes on, because the commits it pushes are the ones you saw, unless one of them changed.
