# Finish or throw away a task

![The sign-off card with its screenshot, then the push window](../assets/clips/push-and-card.gif)

Close a task you pushed, drop one you no longer want, or reset one to start over.

| You want to | Run | Board |
| --- | --- | --- |
| Push it and close it | `kelpie land billing && kelpie push billing` | `p`, then `y` |
| Close it after pushing it some other way: cherry-picked, squashed, or through a pull request | `kelpie done billing` | `d`, then `y` |
| Throw it away when it has no unpushed work | `kelpie drop billing` | `x` |
| Throw it away with its uncommitted work and unpushed commits | `kelpie drop billing --force` | `x`, then confirm the force |
| Stop it for good and put its backlog item back | `kelpie reset billing` | `z` |

Each one frees the worktree for the next task.

`done` refuses while a change on the branch is not on the default branch, and names the missing commits. With the vault, it also waits for the task's note to have its solution and decisions.

Run without a name from inside the task's worktree, these act on that task.
