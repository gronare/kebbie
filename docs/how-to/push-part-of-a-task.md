# Push part of a task

Land a task's first commits while the rest of its branch keeps working.

## From the board

1. Select the task and press `p`. kebbie opens the push window.
2. Move to the last commit you want to land with `↑` `↓`.
3. Press `p`.

kebbie pushes the commits up to and including the selected one. The task stays open, and the rest of its branch is rebased on top of what was pushed.

## From the command line

```bash
kebbie push billing --through 3f2a91c
```

When fetching after the push fails, kebbie says so; run `kebbie land billing` to rebase the rest.

## Before you push

In the push window, `e` edits the selected commit's message, and `s` squashes all commits into one. Only messages change; the code is identical. A commit that arrives after the window opened stops the push.
