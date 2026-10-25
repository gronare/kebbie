# Your first task

![A task started from the board, watched in its Space, ready on the board](assets/clips/getting-started.gif)

In this tutorial you install kebbie, start a task in a small practice repo, watch Claude work on it in its own worktree, and push the result to the repo's default branch. It takes about twenty minutes, most of it installing tools.

You need macOS or Linux and a Claude Code login.

## 1. Install the tools

kebbie runs inside [herdr](https://herdr.dev) and needs these on your `PATH`. Skip any you already have.

On macOS:

```bash
brew install herdr jq fzf neovim curl tig
curl -fsSL https://claude.ai/install.sh | bash
```

On Linux, see [Requirements](reference/requirements.md) for each tool's install command and the versions kebbie needs.

## 2. Install kebbie

Install the herdr plugin at the latest release. The `curl` part asks GitHub for the latest release tag, and herdr clones that tag and installs its prebuilt binary:

```bash
herdr plugin install --ref "$(curl -fsSL https://api.github.com/repos/gronare/kebbie/releases/latest | jq -r .tag_name)" gronare/kebbie
```

herdr shows what the plugin will run and asks you to confirm.

Restart herdr. On startup the plugin creates `~/.local/share/kebbie/current`, a link to where herdr installed it.

## 3. Run setup

```bash
~/.local/share/kebbie/current/target/release/kebbie setup
```

Setup prints one line per step, each starting with `done`, `already`, `skipped` (with the reason) or `to do`:

- it links `~/.local/bin/kebbie` to the binary, and tells you the line to add to your shell profile when `~/.local/bin` is not on your `PATH`;
- it installs the Claude Code plugin from the same folder, so the plugin always matches the binary;
- it shows the four keys it adds to the end of herdr's `config.toml` and writes them only on your `y`: `prefix+space` goes to the lead, `prefix+tab` moves between the board and the lead's chat, `prefix+t` opens the board, `prefix+u` updates kebbie. A key you already use for something else is left alone and named;
- it reloads herdr's config so the keys work, or tells you to restart herdr when it cannot.
- last, the hub, which is optional: it asks whether to set it up now, to see this machine's board on your phone or in a browser, and runs `kebbie hub connect` only on your `y` (see [See your board on your phone](how-to/see-your-board-on-your-phone.md)). `⏎` skips it; you can do it later with `H` on the board.

It ends with `Setup done.` and the warnings and failures from `kebbie doctor`. You can run setup again at any time; it changes only what is missing.

kebbie sends its maintainers error signals when kebbie itself crashes or hits some internal errors: only its version and commit, your OS and CPU architecture, the part of kebbie that failed, the kind of failure and a count, at most once a day, each send shown under messages on the board. A bug report with text is shown to you and sent only on your `y`. To turn the signals off, put this in `~/.config/kebbie/config.toml`:

```toml
[report]
signals = false   # no error signals; text = "ask" (the default) still asks before any bug report
```

[Error signals](how-to/report-a-problem-with-kebbie.md#error-signals) says exactly what one holds.

## 4. Check the machine

```bash
kebbie doctor
```

You see one line per check, each starting with `ok`, `warn` or `fail`:

```text
ok    git                   /usr/bin/git
ok    herdr                 0.9.5 at /opt/homebrew/bin/herdr
ok    claude                /Users/you/.local/bin/claude
...
ok    kebbie on PATH        /Users/you/.local/bin/kebbie → ...
```

Every `warn` and `fail` comes with a `fix:` line. Run the fix and run `kebbie doctor` again until nothing says `fail`.

## 5. Make a practice repo

kebbie pushes to a repo's remote, so give the practice repo a remote of its own on your disk:

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

kebbie creates the lead Space and moves it to the top of herdr's Spaces. On the left is the board, with the Work column, the Backlog column and the status line. On the right is the lead, a Claude that keeps track of your tasks. The Work column is empty.

Press `tab` on the board. Focus moves to the lead's chat. From the chat, `prefix+tab` brings focus back to the board.

Press `h` on the board to hide the chat and give the board the whole width; the lead keeps running. `h` again, or `tab`, brings the chat back.

## 7. Start a task

Open a shell in herdr, then start a task from the practice repo:

```bash
cd ~/code/hello
kebbie start greet "Add a script hello.sh that prints Hello from kebbie, make it executable, and commit it."
```

kebbie prints where the task went. Claude Code has not seen this folder before, so it first asks whether you trust it:

```text
! Started
  next: Once you answer, kebbie types the task's prompt in while the keeper or an open board runs.
  you: Answer greet's question in its Space: whether you trust the repo's folder.
  task: greet
  folder: /Users/you/code/wt1.hello
```

Go to the `greet` Space in herdr's sidebar and answer that you trust the folder. kebbie then types the task's prompt into Claude. In a folder you already trust, the first line is `✓ Started` and Claude gets the prompt at once.

Look at what happened:

- A new folder `~/code/wt1.hello` sits next to `~/code/hello`. It is a git worktree on a new branch `greet`.
- herdr opened a Space labelled `greet`, and Claude is running in it.
- `~/code/hello` is untouched. `git -C ~/code/hello status` is clean.

## 8. Watch it work

Press `prefix+space` to go back to the lead Space.

The board now lists `greet` under **Working**. Claude first makes its worktree runnable with the setup skill, then writes the script and commits it.

When Claude ends its turn it reports where it stands. The row moves to another group, and a `you:` line under it says what you should do next. Select the row with `↑` `↓` and the row opens with the task's report.

When the task reports ready, kebbie checks that each commit has a subject, a blank line and a body, and `greet` moves under **Ready to push**. A read-only reviewer then checks the commits against the rules skill in the background, and the row is marked `reviewer checking rules` until it passes them. You can push before the check is done too.

## 9. Push it

With `greet` selected, press `p`.

kebbie opens the push window. At the top is the task's sign-off card: what changed, where to look, and anything to confirm. Below it are the commits that will land. Use `↑` `↓` to read each commit's full message.

Press `y`.

kebbie fetches, rebases the task onto `origin/main`, pushes the commits to `main`, closes the `greet` Space and gives `wt1.hello` back to the pool. The board's Work column is empty again.

Check the remote:

```bash
git -C ~/code/remotes/hello.git log --oneline main
```

You see Claude's commit on top of `Start hello`. There was no pull request and no extra branch on the remote: the task's commit landed on `main` as it was.

## What you did

You installed kebbie's three parts, opened the board and the lead, ran one task in its own worktree and Space, and pushed it to the default branch after reading its card and commits.

Next:

- Start your next task by telling the lead what you want done, in its chat.
- [Set up a repo's worktree setup](how-to/set-up-worktrees.md) for a real project.
- Read [How kebbie fits together](explanation/architecture.md).
- Look up every key on the board in [the board reference](reference/board.md).
