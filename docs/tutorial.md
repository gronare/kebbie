# Your first task

![A task started from the board, watched in its Space, ready on the board](assets/clips/getting-started.gif)

In this tutorial you install kelpie, start a task in a small practice repo, watch Claude work on it in its own worktree, and push the result to the repo's default branch. It takes about twenty minutes, most of it installing tools.

You need macOS or Linux and a Claude Code login.

## 1. Install the tools

kelpie runs inside [herdr](https://herdr.dev) and needs these on your `PATH`. Skip any you already have.

On macOS:

```bash
brew install herdr jq fzf neovim curl tig
curl -fsSL https://claude.ai/install.sh | bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

On Linux, see [Requirements](reference/requirements.md) for each tool's install command and the versions kelpie needs.

## 2. Install kelpie

Install the herdr plugin. herdr clones kelpie and builds it with `cargo`, which takes a minute or two:

```bash
herdr plugin install gronare/kelpie
```

Restart herdr. On startup the plugin creates `~/.local/share/kelpie/current`, a link to where herdr installed it.

Put the `kelpie` command on your `PATH`:

```bash
mkdir -p ~/.local/bin
ln -s ~/.local/share/kelpie/current/target/release/kelpie ~/.local/bin/kelpie
```

Install the Claude Code plugin from the same folder, so it always matches the binary:

```bash
claude plugin marketplace add ~/.local/share/kelpie/current
claude plugin install kelpie@kelpie
```

## 3. Add the keys

Open herdr's `config.toml` and add these four keys:

```toml
[[keys.command]]
key = "prefix+space"
type = "plugin_action"
command = "kelpie.lead"
description = "kelpie: go to the lead, or back"

[[keys.command]]
key = "prefix+tab"
type = "plugin_action"
command = "kelpie.focus"
description = "kelpie: between the board and the lead's chat"

[[keys.command]]
key = "prefix+t"
type = "plugin_action"
command = "kelpie.board"
description = "kelpie board"

[[keys.command]]
key = "prefix+u"
type = "plugin_action"
command = "kelpie.update"
description = "kelpie: update and reload"
```

Restart herdr so it reads them.

## 4. Check the machine

```bash
kelpie doctor
```

You see one line per check, each starting with `ok`, `warn` or `fail`:

```text
ok    git                   /usr/bin/git
ok    herdr                 0.9.5 at /opt/homebrew/bin/herdr
ok    claude                /Users/you/.local/bin/claude
...
ok    kelpie on PATH        /Users/you/.local/bin/kelpie → ...
```

Every `warn` and `fail` comes with a `fix:` line. Run the fix and run `kelpie doctor` again until nothing says `fail`.

## 5. Make a practice repo

kelpie pushes to a repo's remote, so give the practice repo a remote of its own on your disk:

```bash
mkdir -p ~/code/remotes && git init --bare ~/code/remotes/hello.git
git init ~/code/hello && cd ~/code/hello
echo "# hello" > README.md
git add README.md && git commit -m "Start hello"
git branch -M main
git remote add origin ~/code/remotes/hello.git
git push -u origin main
```

## 6. Open the lead

In herdr, press `prefix+space`.

kelpie creates the lead Space and moves it to the top of herdr's Spaces. On the left is the board, with the Work column, the Backlog column and the status line. On the right is the lead, a Claude that keeps track of your tasks. The Work column is empty.

Press `tab` on the board. Focus moves to the lead's chat. From the chat, `prefix+tab` brings focus back to the board.

Press `h` on the board to hide the chat and give the board the whole width; the lead keeps running. `h` again, or `tab`, brings the chat back.

## 7. Start a task

Open a shell in herdr, then start a task from the practice repo:

```bash
cd ~/code/hello
kelpie start greet "Add a script hello.sh that prints Hello from kelpie, make it executable, and commit it."
```

kelpie prints where the task went:

```text
Started greet in /Users/you/code/wt1.hello.
```

Look at what happened:

- A new folder `~/code/wt1.hello` sits next to `~/code/hello`. It is a git worktree on a new branch `greet`.
- herdr opened a Space labelled `greet`, and Claude is running in it.
- `~/code/hello` is untouched. `git -C ~/code/hello status` is clean.

## 8. Watch it work

Press `prefix+space` to go back to the lead Space.

The board now lists `greet` under **Working**. Claude first makes its worktree runnable with the setup skill, then writes the script and commits it.

When Claude ends its turn it reports where it stands. The row moves to another group, and a `you:` line under it says what you should do next. Select the row with `↑` `↓` and the row opens with the task's report.

The lead checks the task's commits against the rules skill. Once it approves them, `greet` moves to **Ready to push**. If you pick it up before the lead has looked, it waits under **Needs you** marked "rules not checked by the lead"; you can push from there too.

## 9. Push it

With `greet` selected, press `p`.

kelpie rebases the task onto `origin/main`, then opens the push window. At the top is the task's sign-off card: what changed, where to look, and anything to confirm. Below it are the commits that will land. Use `↑` `↓` to read each commit's full message.

Press `y`.

kelpie pushes the commits to `main`, closes the `greet` Space and gives `wt1.hello` back to the pool. The board's Work column is empty again.

Check the remote:

```bash
git -C ~/code/remotes/hello.git log --oneline main
```

You see Claude's commit on top of `Start hello`. There was no pull request and no extra branch on the remote: the task's commit landed on `main` as it was.

## What you did

You installed kelpie's three parts, opened the board and the lead, ran one task in its own worktree and Space, and pushed it to the default branch after reading its card and commits.

Next:

- Start your next task by telling the lead what you want done, in its chat.
- [Set up a repo's worktree setup](how-to/set-up-worktrees.md) for a real project.
- Read [How kelpie fits together](explanation/architecture.md).
- Look up every key on the board in [the board reference](reference/board.md).
