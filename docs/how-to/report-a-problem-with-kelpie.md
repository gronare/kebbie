# Report a problem with kelpie

Keep what kelpie got wrong, or could not do, on your machine, see after an update which of it may be fixed, and send one to kelpie's developers when you choose.

## Save it

The lead proposes saving an issue when kelpie cost something in a session: an error, a workaround, a missing capability. Say yes and it saves one. A task tells the lead instead of asking you.

To save one yourself:

```bash
kelpie issue add --kind bug --title "push hides git's error when the rebase fails" - <<'EOF'
What happened: kelpie push stopped and printed nothing.
What I expected: git's error message.
How to reproduce: 1. Start a task. 2. Make the rebase conflict. 3. Push.
EOF
```

`--kind wish` is for something kelpie lacks. kelpie records its version, commit and the date.

## See what you saved

```bash
kelpie issues          # open ones
kelpie issues --all    # every status
kelpie issue show <id>
```

## After an update

`kelpie update` prints a `kelpie issues:` line when issues are open, and asks the lead which ones the new commits likely fix. The lead tells you which to try again. Then:

```bash
kelpie issue retry <id>          # worth trying again
kelpie issue close <id> fixed    # or wontfix, duplicate
```

With `gh` installed and logged in, the update also finds the issues you sent on GitHub, links them, and marks one `retry` once it is closed there.

## Send it to GitHub

This is yours alone. kelpie refuses `kelpie issue send` to the lead and tasks: the lead logs issues with `kelpie issue add`, and a task tells the lead.

```bash
kelpie issue send <id>
```

kelpie prints the text and opens a prefilled issue in kelpie's GitHub repo in your browser. kelpie's repo is public: read the text for project names, paths and anything private before you press Submit. Nothing is sent until you do. A text too long for the URL is shortened, and says so; paste the rest from `kelpie issue show <id>`.

If the browser does not open (over SSH, or on Linux without a display), open the printed URL yourself. `kelpie issue link <id> <number>` ties the saved issue to its GitHub issue right away.

See also: [`kelpie issues`](../reference/commands.md#kelpie-issues), [The issue file](../reference/files.md#the-issue-file).
