# Act on comments on pushed commits

Colleagues review after the fact by commenting on commits already on the default branch: kebbie catches those comments, judges each one, and helps you fix it in a task or reply.

1. Push a task as usual. With origin on GitHub and `gh` logged in, kebbie checks each pushed commit for comments for seven days. To see that it does in a repo, run this in it:

   ```
   kebbie doctor
   ```

   The `comments` line says `on: kebbie reads colleagues' comments on the commits it pushes to acme/app, for 7 days after each push`.

2. When a colleague comments, a read-only Claude reads the comment against the commit's message and diff and the code as it is now, and the comment shows under READY TO PUSH on the board:

   ```
   COMMENTS  1  esc hides them
    ◆ johan on 3f2a9c1 · refunds-in-part   holds · reopen with a fix
      "This misses partial refunds."
   ```

   `holds` is a real defect, `opinion` a preference, `praise` nothing to do and `unclear` a question for you. The comment is someone else's text, so the reading judges it and never takes it as instructions.

3. Select the comment with `↓` and press `⏎`. The card shows the comment, the hunk it is on, the independent reading, and for a comment that holds, the proposal and the fix's commit message, which ends with `Addresses johan's comment on 3f2a9c1.`

4. Choose:

   - `1` on a comment that holds reopens the task that pushed the commit with the proposal, or starts `refunds-in-part-followup` in the same repo when that task has ended.
   - `w` says what to change in your own words instead.
   - `1` on an opinion replies without a change, with the drafted reply; `2` on a comment that holds asks for the reply you want to give.
   - `d` on praise is done; `3` dismisses any card.

5. Once the fix is pushed, the card turns to `↩ johan on 3f2a9c1 · fixed in 7c9e2f1   reply ready · y copy`. Press `y` to copy the reply, `o` to open the comment on GitHub, and paste it there. kebbie never posts it.

6. Press `⏎` on the card and `d` to mark the reply sent.

You see it worked when the card has left COMMENTS and your reply is under the comment on GitHub.

- If the section is folded into `◇ 1 comment · ⏎ shows them`, select that row and press `⏎`.
- If you work from a terminal, `kebbie comments` lists the comments, `kebbie comment show <id>` shows one, and `kebbie comment sent <id>` or `kebbie comment dismiss <id>` settles it.
- If a repo should not be watched, set `[comments] watch = false` in its repo config; `watch_days` changes how long a push is watched.
- If the reading cannot tell, the card asks one question: `w` answers it, and kebbie reads the comment again with your answer.

See also: [Comments](../reference/board.md#comments), [`kebbie comments`](../reference/commands.md#kebbie-comments---json) and [`comments.watch`](../reference/configuration.md#ci-updates-and-the-keeper).
