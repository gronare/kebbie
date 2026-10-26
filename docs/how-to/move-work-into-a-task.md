# Move work into a task

Turn changes you started in the main checkout into a task, so the main checkout is clean again.

## From inside Claude

1. In the Claude session that did the work, run:

   ```text
   /kebbie:move-to-worktree billing
   ```

Claude claims the files and commits it made, moves them and its own pane into a worktree and Space named `billing`, runs the setup skill there and continues.

## From the command line

1. From the main checkout, name the task and the uncommitted files to take:

   ```bash
   kebbie move billing app/models/invoice.rb app/models/line.rb
   ```

   kebbie takes branch `billing` if it exists, else the unpushed commits, plus the files you name, and opens the worktree as a Space.

2. Run the setup skill that the last line names in the new worktree, or ask the Claude you moved to run it.

- If you want a herdr pane to move along, add `--pane <id>`.
- If you want to stay where you are, add `--no-focus`.
