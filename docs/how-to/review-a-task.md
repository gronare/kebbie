# Review a task's changes

![A comment on a line in the review split, sent to Claude](../assets/clips/review-in-nvim.gif)

Read a task's diff next to its Claude and send it comments line by line.

1. Select the task on the board and press `r`. Or, in the task's Claude, run `/kelpie:review-split`.

   A split opens next to Claude with the task's files in review order.

2. Press `Enter` on a file to open its diff in nvim.

3. Press `c` on a line, or on a visual selection, and write your comment. It is typed into Claude's input box as `path:line — text`, not sent.

4. Press `r` to mark the file read and go back to the list. Repeat for the next file.

5. Switch to Claude's pane and press Enter to send all your comments at once.

To edit a file yourself, press `o` in the diff. Save, then `ctrl-q` back to the diff; Claude is told which lines you edited.

To open the split on every task automatically, turn on `auto_open`: `/plugin` → Installed → kelpie → Configure.

Every key: [Review split](../reference/review-split.md).
