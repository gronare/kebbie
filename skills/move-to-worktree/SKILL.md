---
name: move-to-worktree
description: Move this session's work in progress out of the main checkout into its own git worktree and herdr Space, and continue there. Use when the user runs /move-to-worktree or asks to move the current work into a worktree.
argument-hint: "<task name>"
disable-model-invocation: true
---

# Move to worktree

`kelpie` is `${KELPIE_BIN:-$HOME/.local/share/kelpie/current/target/release/kelpie}`.

1. Stop if `HERDR_ENV` is not `1` or `kelpie` is missing, and say why.
2. Work out what is yours, from the main checkout:
   - If your work is on its own branch, the task name is that branch.
   - If it is on the default branch, your unpushed commits (`git log origin/HEAD..HEAD`) move with it. If any of them is not yours, stop and tell the user.
   - Your uncommitted files: only the ones you changed in this session. Leave the rest; ask if unsure.
3. Run `kelpie move <name> --pane "$HERDR_PANE_ID" <your files>` from the main checkout. Add `--no-focus` if the user wants to stay where they are.
4. Switch into the worktree it printed with EnterWorktree and `path`.
5. Reply in two lines: where the work is now (worktree and branch), and what stayed behind in the main checkout.

In the worktree, `/kelpie:review-split` reviews everything since the branch left the default branch, specs use the worktree's own test database, and the development database is still shared, so do not run migrations there without asking.
