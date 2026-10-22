# Try a task before pushing

Put a ready task's commits on your main checkout's local `main`, run the real thing from there, and push main once it is right. For a repo you run from its main checkout, above all kelpie itself when you run kelpie from your own clone (see [Develop kelpie](develop-kelpie.md)).

## Turn it on for the repo

It is off unless the repo turns it on, in its repo config, named after the main checkout's folder:

```toml
# ~/.config/kelpie/repos/kelpie.toml
[worktree]
stage_on_main = true
```

kelpie's own repo is the one to turn it on for: a task's commits reach the kelpie you run only through main.

## Stage a task

1. Select a task under Ready to push and press `M` (Put on local main to try it, in its options).
2. kelpie cherry-picks its commits onto local `main`, unpushed. The row says `on local main · waiting for your push`.
3. In kelpie's own repo on the main channel, kelpie rebuilds, and the board and keeper restart onto the new binary. In any other repo, restart what you run from the main checkout.

From the command line:

```bash
kelpie stage billing
```

Stage more tasks the same way; each goes on top of the ones before it, in the order you stage them. When one conflicts with what is already staged, main stays as it was and the task's Claude rebases it onto local main, and kelpie stages it again by itself once it reports ready.

Keep your own commits off local main while tasks are staged: kelpie never rewrites them, so it refuses to stage or unstage, and leaves main as it is after other pushes, until you push or move them.

## Take one off

Press `M` on the staged task, or run `kelpie unstage billing`. Only its commits leave main; the other staged tasks stay, and its branch keeps its commits.

## Push main

When everything staged is right, push main: `git push` in the main checkout, or `p` on the Areas page. The keeper closes each staged task once its commits are on origin, within a minute.

A staged task is never pushed on its own: `p` on it, or `kelpie push`, says `it is on local main; push main`. Take it off first to push it through the push line instead.
