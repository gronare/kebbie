# Push part of a task

Land a task's first commits while the rest of its branch keeps working.

## From the board

1. Select the task.
2. Press `p` to open the push window.
3. Move to the last commit you want to land with `↑` `↓`.
4. Press `p`.

kebbie pushes the commits up to and including the selected one. The task stays open, and the rest of its branch is rebased on top of what was pushed.

## From the command line

1. Push through the last commit you want to land:

   ```bash
   kebbie push billing --through 3f2a91c
   ```

- If fetching after the push fails, kebbie says so. Run `kebbie land billing` to rebase the rest.

## Before you push

In the push window:

| Key | What it does |
| --- | --- |
| `e` | Edits the selected commit's message. |
| `s` | Squashes all commits into one. |

Only messages change; the code is identical. A commit that arrives after the window opened stops the push.
