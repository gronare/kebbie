---
name: move-to-worktree
description: Move this session's work in progress out of the main checkout into its own git worktree and herdr Space, and continue there.
argument-hint: "<task name>"
disable-model-invocation: true
---

# Move to worktree

`kebbie` is `${KEBBIE_BIN:-$HOME/.local/share/kebbie/current/target/release/kebbie}`. Steps 1 to 3 run from the main checkout.

1. **Check.** `HERDR_ENV` is `1` and `kebbie` exists. When one fails, say which and stop.
2. **Claim what is yours.** Done when you have the task name and your files, or have stopped:
   - Work on its own branch: the task name is that branch.
   - Work on the default branch: the task name is `$ARGUMENTS`, and your unpushed commits (`git log origin/HEAD..HEAD`) move with it. When one of them is another session's, stop and tell the user.
   - Uncommitted files: name only the ones you changed in this session; the rest stay in the main checkout. Ask the user about any you are unsure of.
3. **Move.** Run `kebbie move <name> --pane "$HERDR_PANE_ID" <your files>`, adding `--no-focus` when the user wants to stay where they are. Done when it prints `Moved <task> to <path>.`
4. **Enter.** Switch into that path with EnterWorktree and `path`.
5. **Set up.** The last line it printed, `Set it up: run the skill <skill> in <path>.`, names the setup skill: run it and follow it to its end. It makes the worktree runnable without touching the main checkout.
6. **Reply** in two lines: where the work is now (worktree and branch), and what stayed behind in the main checkout.

In the worktree, `/kebbie:review-split` reviews everything since the branch left the default branch.
