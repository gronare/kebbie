# Try a task before pushing

Put a ready task's commits on your main checkout's local `main`, run the real thing from there, and push main once it is right.

It suits a repo you run from its main checkout, above all kebbie itself when you run kebbie from your own clone (see [Develop kebbie](develop-kebbie.md)). A task's commits reach the kebbie you run only through main.

## Stage a task

1. Turn it on in the repo config, named after the main checkout's folder. It is off unless the repo turns it on:

   ```toml
   # ~/.config/kebbie/repos/kebbie.toml
   [worktree]
   stage_on_main = true
   ```

2. Select a task under Ready to push and press `M` (Put on local main to try it, in its options). Or from the command line:

   ```bash
   kebbie stage billing
   ```

   kebbie cherry-picks its commits onto local `main`, unpushed.

3. Restart what you run from the main checkout. When the repo is kebbie's install root and the machine follows the `main` channel, there is nothing to restart: kebbie rebuilds, and the board and keeper restart onto the new binary.

4. Check the row. It says `on local main · waiting for your push`.

- If you stage more tasks, each goes on top of the ones before it, in the order you stage them.
- If a task conflicts with what is already staged, main stays as it was. The task's Claude rebases it onto local main, and kebbie stages it again by itself once it reports ready.
- If you have your own commits on local main, kebbie refuses to stage or unstage, and leaves main as it is after other pushes, until you push or move them. It never rewrites them.

## Take a task off

1. Press `M` on the staged task, or run:

   ```bash
   kebbie unstage billing
   ```

   Only its commits leave main. The other staged tasks stay, and its branch keeps its commits.

- If a task staged after it builds on its commits, kebbie refuses. Take that one off first.

## Push main

1. When everything staged is right, push main: `git push` in the main checkout, or `p` on the Areas page.

2. Wait up to a minute. The keeper closes each staged task once its commits are on origin.

- If you press `p` on a staged task or run `kebbie push`, it says `it is on local main; push main`. A staged task is never pushed on its own. Take it off first to push it through the push line instead.
