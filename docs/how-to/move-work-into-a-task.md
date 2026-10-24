# Move work into a task

You started changing code in the main checkout and it should be a task. Move it, so the main checkout is clean again.

## From inside Claude

In the Claude session that did the work:

```text
/kebbie:move-to-worktree billing
```

Claude claims the files and commits it made, moves them and its own pane into a worktree and Space named `billing`, runs the setup skill there and continues.

## From the command line

From the main checkout:

```bash
kebbie move billing app/models/invoice.rb app/models/line.rb
```

kebbie takes branch `billing` if it exists, else the unpushed commits, plus the uncommitted files you name, and opens the worktree as a Space. Add `--pane <id>` to move that herdr pane along, or `--no-focus` to stay where you are.

Its last line names the setup skill to run in the new worktree. Run it there, or ask the Claude you moved to.
