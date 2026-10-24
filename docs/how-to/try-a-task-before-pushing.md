# Try a task before pushing

Put a ready task's commits on your main checkout's local `main`, run the real thing from there, and push main once it is right. For a repo you run from its main checkout, above all kebbie itself when you run kebbie from your own clone (see [Develop kebbie](develop-kebbie.md)).

## Turn it on for the repo

It is off unless the repo turns it on, in its repo config, named after the main checkout's folder:

```toml
# ~/.config/kebbie/repos/kebbie.toml
[worktree]
stage_on_main = true
```

kebbie's own repo is the one to turn it on for: a task's commits reach the kebbie you run only through main.

## Stage a task

1. Select a task under Ready to push and press `M` (Put on local main to try it, in its options).
2. kebbie cherry-picks its commits onto local `main`, unpushed. The row says `on local main · waiting for your push`.
3. When the repo is kebbie's install root and the machine follows the `main` channel, kebbie rebuilds, and the board and keeper restart onto the new binary. In any other repo, restart what you run from the main checkout.

From the command line:

```bash
kebbie stage billing
```

Stage more tasks the same way; each goes on top of the ones before it, in the order you stage them. When one conflicts with what is already staged, main stays as it was and the task's Claude rebases it onto local main, and kebbie stages it again by itself once it reports ready.

Keep your own commits off local main while tasks are staged: kebbie never rewrites them, so it refuses to stage or unstage, and leaves main as it is after other pushes, until you push or move them.

## Take one off

Press `M` on the staged task, or run `kebbie unstage billing`. Only its commits leave main; the other staged tasks stay, and its branch keeps its commits. When a task staged after it builds on its commits, kebbie refuses: take that one off first.

## Push main

When everything staged is right, push main: `git push` in the main checkout, or `p` on the Areas page. The keeper closes each staged task once its commits are on origin, within a minute.

A staged task is never pushed on its own: `p` on it, or `kebbie push`, says `it is on local main; push main`. Take it off first to push it through the push line instead.
