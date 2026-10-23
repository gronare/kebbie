# Report a problem with kelpie

Keep what kelpie got wrong, or could not do, on your machine, see after an update which of it may be fixed, and send one to kelpie's maintainers when you choose.

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

You do nothing. After `kelpie update`, the keeper rechecks each of your open issues that was filed on older code, one at a time in the background. A read-only Claude reads the issue, every commit since the build it was filed on with its full message, and kelpie's source as it is now, and answers fixed, not fixed or unclear. It answers fixed only when a commit plainly addresses the issue and the code shows it.

- **Fixed:** kelpie closes the issue as `fixed` and records `fixed in <version> (<sha>), checked automatically` in it.
- **Not fixed or unclear:** the issue stays open, and is not checked again until the next update.
- **No answer** (Claude failed, or gave none within 15 minutes): the issue stays open, with one line in the keeper's log.

Nobody asks you anything. The board shows one line under Work as soon as the first recheck closes an issue, such as `✓ kelpie 0.26.3 fixed 2 of your issues`, with the titles below it, and adds to it as the other rechecks finish. `i` lists them all and clears the line, once any questions and report offers are dealt with; a fix that lands later shows again. The line also goes by itself three days after the last fix, and an issue you reopen leaves it. If a recheck closes one while `kelpie update` still runs, the update's output lists it too. `kelpie issues --all` shows every closed issue with its last recorded line.

When a fixed issue comes back, reopen it rather than filing a new one; the lead does this when what you hit matches a closed issue:

```bash
kelpie issue reopen <id>         # records "came back on <version>"
```

The manual steps still work:

```bash
kelpie issue retry <id>          # worth trying again
kelpie issue close <id> fixed    # or wontfix, duplicate
```

With `gh` installed and logged in, the update also finds the issues you sent on GitHub, links them, and marks one `retry` once it is closed there.

## Send it to kelpie's maintainers

When kelpie or the lead saves a bug about kelpie, the board lists it under Work, below the tasks, under **Send to kelpie's maintainers?**. Press `i` (after any questions waiting): the board shows the report exactly as it would go out and asks `y / n`. A long one scrolls with `↓` and `pgdn`, and `y` counts only once you have read to its end.

- `y` sends it, as long as the issue is unchanged since you read it; an edit in between sends nothing and asks you to read it again. kelpie answers the reports endpoint's proof-of-work challenge (about a second), signs the report with this install's key and records the report's id in the issue, so it is never sent twice.
- `n` keeps it on this machine only, and the board stops offering it.
- `esc` decides later.

The report holds the title, the text, kelpie's version and commit, and the OS and architecture. Before you see it, kelpie replaces paths under any home (`/Users`, `/home`, `/var/home`) with `~` and your user name in any other path with `<user>`, the names of the repos it works in with `<repo>` (also inside names like `<repo>-api`), remote and other URLs with `<remote>` or `<url>`, email addresses, host names and IP addresses with `<email>`, `<host>` and `<ip>`, branch names such as `feature/…` with `<branch>`, and anything shaped like a token, a key id, a long hex string that is not one of kelpie's own commits, or a secret assignment with `<token>` or `<redacted>`. Terminal escapes and control characters are removed. No environment, file or repo content is added. Read it anyway: the text is what the lead or you wrote.

From a terminal:

```bash
kelpie issue report <id>
```

It prints the same text and asks `y/N`. Only you can send: kelpie refuses it, and the board's `y`, to the lead, tasks and sandboxed processes. Wishes are not offered; send one with `kelpie issue send` below.

The install key is made on the first report and kept in `report/install-key.json` in the state directory, readable by you alone. The project's maintainers see that key as the report's sender; a key they have not seen before waits until they let it through, so the first report from a new install may take a while to be read.

`[report] enabled = false` in config.toml hides the offer; `kelpie issue report <id>` still works. `[report] url` sends reports to another endpoint, such as a [kelpie hub](https://github.com/gronare/kelpie-hub) you run. See [Configuration](../reference/configuration.md).

### Receiving reports

If you run the hub that takes your users' reports, mint a token on it with `kelpie-hub admin token --name <this machine>` and set it in your own config.toml as `[report] inbox_token`. The keeper then fetches the reports the hub has let through every 10 minutes, files each as a kelpie issue on your machine, with the id `r<report id>` and the sender's title, and acknowledges it on the hub, once per report. `kelpie issues` and `kelpie issue show` mark such an issue `[report <id>, untrusted]` and show its text fenced as data; it is never rechecked after an update. `kelpie doctor` counts the reports still waiting in quarantine; `kelpie-hub admin reports --quarantine` lists them and `kelpie-hub admin reports allow <key>` lets a sender through. The token belongs in config.toml only; a repo config with `[report]` is refused.

## Send it to GitHub

This is yours alone. kelpie refuses `kelpie issue send` to the lead and tasks: the lead logs issues with `kelpie issue add`, and a task tells the lead.

```bash
kelpie issue send <id>
```

kelpie prints the text and opens a prefilled issue in kelpie's GitHub repo in your browser. kelpie's repo is public: read the text for project names, paths and anything private before you press Submit. Nothing is sent until you do. A text too long for the URL is shortened, and says so; paste the rest from `kelpie issue show <id>`.

If the browser does not open (over SSH, or on Linux without a display), open the printed URL yourself. `kelpie issue link <id> <number>` ties the saved issue to its GitHub issue right away.

See also: [`kelpie issues`](../reference/commands.md#kelpie-issues), [The issue file](../reference/files.md#the-issue-file).
