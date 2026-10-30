# Report a problem with kebbie

Keep what kebbie got wrong, or could not do, on your machine, and send a bug to kebbie's maintainers when you choose.

- What you save stays on your machine. After an update, kebbie shows which of it may be fixed.
- A bug goes to kebbie's issues endpoint, from the board's bugs to send or `kebbie issue report`.
- A GitHub issue is optional, and only ever one you open yourself.
- Separately, kebbie counts its own crashes and internal errors and sends the maintainers those counts, with no text at all.
- Once a day, kebbie sends an install signal so the maintainers can count active installs, with no text and nothing about your work.

## Save it

When kebbie costs something in a session (an error, a workaround, a missing capability), the lead notes it in one line with `kebbie issue hold` and carries on with your work.

It asks about it later, at a calm moment:

- what you asked for is done,
- nothing on the board waits on you: no open question, nothing under Needs you, away mode off,
- your latest message asks for nothing new.

Then the question is the lead's whole reply, one yes/no line, and it saves the issue only when you say yes.

One line is held at a time, so several things that went wrong come to you as one question. The held line stays on this machine and survives a restart of the lead. It is never an issue, offered on the board or sent anywhere.

A task tells the lead instead of asking you. Once saved, an issue is the keeper's to look after: the lead leaves kebbie issues out of its briefings and of what it asks you to decide, and takes up closing or retrying one when you ask about it.

### Stop the lead proposing them

Set this in config.toml. It takes effect when the lead next restarts (`L` on the board), and the lead then holds nothing and saves one only when you ask it to.

```toml
[issues]
propose = false
```

### Save one yourself

```bash
kebbie issue add --kind bug --title "push hides git's error when the rebase fails" - <<'EOF'
What happened: kebbie push stopped and printed nothing.
What I expected: git's error message.
How to reproduce: 1. Start a task. 2. Make the rebase conflict. 3. Push.
EOF
```

`--kind wish` is for something kebbie lacks. kebbie records its version, commit and the date.

## See what you saved

```bash
kebbie issues          # open ones
kebbie issues --all    # every status
kebbie issue show <id>
```

## After an update

You do nothing. After `kebbie update`, the keeper rechecks each open bug and wish that was filed on older code, one at a time in the background.

Older code means a commit this install's history holds and the new build descends from. An issue filed on a build the history cannot place, such as one built outside git, is not rechecked, and you close it yourself.

A read-only Claude reads the issue, every commit since the build it was filed on with its full message, and kebbie's source as it is now. It answers fixed, not fixed or unclear, and answers fixed only when a commit plainly addresses the issue and the code shows it.

| Answer | What happens |
| --- | --- |
| Fixed | kebbie closes the issue as `fixed` and records `fixed in <version> (<sha>), checked automatically` in it. |
| Not fixed or unclear | The issue stays open, and is not checked again until the next update. |
| No answer (Claude failed, or gave none within 15 minutes) | The issue stays open, with one line in `keeper.log` in the state directory, also when an open board runs the routines with `[keeper] enabled = false`. |

### See what got fixed

Nobody asks you anything. The board shows one line under Work as soon as the first recheck closes an issue, such as `✓ Fixed in kebbie 0.26.3: 2 bugs you ran into`, with the titles below it. It adds to the line as the other rechecks finish, and a fixed wish counts as `1 wish you made`.

- `i` shows the list and clears the line, once any questions are answered. A fix that lands later shows again.
- The line also goes by itself three days after the last fix, and an issue you reopen leaves it.
- `kebbie issues --all` shows every closed issue with its last recorded line.

### When a fixed issue comes back

Reopen it rather than filing a new one; the lead proposes this when what you hit matches a closed issue. A reopened bug is offered for sending again, even one sent or kept here before.

```bash
kebbie issue reopen <id>         # records "came back on <version>"
```

The manual steps still work:

```bash
kebbie issue retry <id>          # worth trying again
kebbie issue close <id> fixed    # or wontfix, duplicate
```

With `gh` installed and logged in, the update also finds the issues you sent on GitHub, links them, and marks one `retry` once it is closed there, unless the issue came back after that.

## Send it to kebbie's maintainers

When you or the lead saves a bug in kebbie you ran into, the board's header counts it, such as `! 2 bugs to send`, and the list waits under Work, below the tasks, folded into one row: `! 2 bugs to send · ⏎ shows them`.

1. Select the row with `↓` from the last task, or a click, and press `⏎`. The list opens under **Send to kebbie's maintainers?**, with the first bug selected.
2. Pick a bug with `↑` `↓` or a click.
3. Press `⏎`, or click it again. kebbie shows the report exactly as it would go out and asks `y / n`.
4. Read it to its end. A long one scrolls with `↓` and `pgdn`, and `y` counts only once you have read to its end.
5. Answer:

   | Key | What it does |
   | --- | --- |
   | `y` | Sends it, as long as the issue is unchanged since you read it; an edit in between sends nothing and asks you to read it again. kebbie answers the reports endpoint's proof-of-work challenge (about a second), signs the report with this install's key and records the report's id in the issue, so it is never sent twice. |
   | `n` | Keeps it on this machine only, and the board stops offering it. `n` in the list does the same for the selected bug without opening it. |
   | `esc` | Decides later. |

Every report sent, from the board, from `kebbie issue report` or unasked, adds a line under messages on the board: `Sent bug <id> to kebbie's maintainers as report <report>: "<title>", redacted; kebbie issue report <id> shows the text that went.`

- If you want back to the tasks, press `esc` in the list to fold it into its row, then `esc` or `↑` again. The board remembers whether the list shows.
- If you work in a terminal, run `kebbie issue report <id>`. It prints the same text and asks `y/N`.

### What the report holds

The report holds the title, the text, kebbie's version and commit, and the OS and architecture. No environment, file or repo content is added. Before you see it, kebbie replaces:

| In the text | Becomes |
| --- | --- |
| Paths under any home (`/Users`, `/home`, `/var/home`) | `~` |
| Your user name in any other path | `<user>` |
| The names of the repos it works in, also inside names like `<repo>-api` | `<repo>` |
| Remote and other URLs | `<remote>` or `<url>` |
| Email addresses, host names and IP addresses | `<email>`, `<host>` and `<ip>` |
| Branch names such as `feature/…` | `<branch>` |
| Anything shaped like a token, a key id, a long hex string that is not one of kebbie's own commits, or a secret assignment | `<token>` or `<redacted>` |
| Terminal escapes and control characters | removed |

> **Warning:** Read it anyway: the text is what the lead or you wrote.

### Who sends and what is offered

- Only you can send: kebbie refuses it, and the board's `y`, to the lead, tasks and sandboxed processes.
- Wishes are not offered; open one on GitHub yourself if you like, as below.
- A bug already sent with `kebbie issue send`, linked to a GitHub issue, closed or kept here is not offered either, until it is reopened.
- The install key is made on the first report and kept in `report/install-key.json` in the state directory, readable by you alone. The project's maintainers see that key as the report's sender. A key they have not seen before waits until they let it through, so the first report from a new install may take a while to be read.

### Choose how bugs go out

`[report] text` in config.toml decides:

| Value | What happens |
| --- | --- |
| `"ask"` (default) | As above, every time. |
| `"always"` | kebbie sends each bug you or the lead saves within a minute, redacted the same way, without asking. Each send is a line under messages, so you still see what went. |
| `"never"` | The board never offers one; `kebbie issue report <id>` still sends one when you ask. |

> **Note:** The `"always"` key alone does nothing. Run `kebbie issue report --always` in your own terminal to arm it, and `kebbie issue report --ask` to go back to being asked. The lead can write config.toml but cannot arm it; `kebbie doctor` says when the key is set and not armed.

## Error signals

When kebbie itself fails, it counts it: a panic anywhere in kebbie, and some internal errors, where a check in kebbie's code that should never fail did or one of its own state files does not read back. Your errors are not counted: a bad config, the network being down or `gh` missing stay on your screen.

Once a day at most, the keeper sends the counts since the last send. Each signal holds exactly:

- kebbie's version and commit,
- the OS and CPU architecture, such as `macos aarch64`,
- the component: a fixed name from kebbie's own code, such as `keeper.push` or `board.draw`; a panic outside kebbie's code is `dependency`,
- the error kind: `panic`, `internal-error` or `corrupt-state`,
- how many times it happened.

Never an error message, a path, a repo, task, branch or host name, or anything from your work: the format has no field for text, and the endpoint refuses one that carries any. It is signed with the same install key as reports.

Each send adds a line under messages on the board, such as `Sent kebbie's maintainers an error signal, a count of kebbie's own failures: keeper.push internal-error (×3); with kebbie 0.26.3 (16983b9) on macos aarch64 and nothing else. [report] signals = false in config.toml turns these off.`

Signals are on by default. To turn them off, in config.toml:

```toml
[report]
signals = false   # sends no error signals and drops what was counted
```

`kebbie doctor`'s `maintainers` line shows both choices, and `kebbie config` lists `report.signals` and `report.text`. A config with the older `[report] enabled = false` keeps both off.

### The install signal

So the maintainers can count how many installs are active, per version and OS, the keeper sends one install signal a day, at most once per UTC day. It holds exactly:

- a random id made for this install alone, 32 hexadecimal characters, kept in `report/install-id` in the state directory; it is not derived from your machine, your user or anything else,
- kebbie's version,
- the OS and CPU architecture, such as `macos aarch64`,
- the update channel, `release` or `main`.

Nothing else: no repo, task, branch, path or host name, and no count of anything you do. It is not signed with the install key, so it cannot be tied to your reports or error signals. A send that fails is dropped, not tried again that day, and it never holds up anything else. It adds no line under messages; `kebbie doctor`'s `maintainers` line names it.

`[report] signals = false` turns it off along with the error signals and forgets the install id, so turning signals on again makes a new one.

## Or open a GitHub issue yourself

The report above is how a bug reaches kebbie's maintainers. A GitHub issue is an optional alternative, and nothing in kebbie opens one for you: not the keeper, not an update, not the lead or a task.

kebbie refuses `kebbie issue send` and `kebbie issue link` to the lead, tasks and sandboxed processes. The lead never suggests a GitHub issue unless you ask for one; then it tells you to run this yourself.

1. Open the issue:

   ```bash
   kebbie issue send <id>
   ```

   kebbie prints the text and opens a prefilled issue in kebbie's GitHub repo in your browser.

2. Read the text for project names, paths and anything private: kebbie's repo is public.
3. Press Submit. Nothing is sent until you do.

- If the text is too long for the URL, kebbie shortens it and says so; paste the rest from `kebbie issue show <id>`.
- If the browser does not open (over SSH, or on Linux without a display), open the printed URL yourself.
- If you want to tie the saved issue to its GitHub issue right away, run `kebbie issue link <id> <number>`.

See also: [`kebbie issues`](../reference/commands.md#kebbie-issues), [The issue file](../reference/files.md#the-issue-file).
