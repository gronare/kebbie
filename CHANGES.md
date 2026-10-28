# Changes

What each kebbie release brings, one line per change, as you would tell someone who uses kebbie. The board's update window shows the entries of the update it installed; scripts/release.sh moves the Unreleased entries under the new version.

## Unreleased

### What's new
- kebbie setup opens your board when it is done: any key once it has applied, or after h has connected the hub
- kebbie:work-rules cuts a branch into steps in the order they go out: preparatory commits such as a migration, an interface, a move or a rename first, the change itself last and small, each one finished and pushable on its own
- With task.steps = true, off by default, a task lists its steps with kebbie steps before its first line of code, one commit each, keeps building the next one while you accept and push the last, and lists them again when you say the split is wrong; kebbie steps shows each one as planned, in progress, committed or accepted, with the pushed ones as a count
- kebbie accept <task> <sha> marks one of a task's commits as accepted by you, whatever the setting; it holds through a clean rebase or a reworded message, a changed line takes it back, and --withdraw takes it back too
- A task in steps shows them under its row on the board, with N to accept and the pushed ones as a count; the status line counts the commits waiting for your accept, and the push window marks accepted commits and opens on the last of them, so p pushes what you accepted while the task keeps working
- kebbie push --through, and p in the push window, leave the task's worktree alone: uncommitted work no longer stops it, the commits go out rebased in a worktree of kebbie's own, and the task's Claude rebases onto the new default branch when its work is committed; a commit it makes meanwhile no longer stops the push
- kebbie accepted <task> [--json] lists which of a task's commits are accepted, and a task may run it
- kebbie steps refuses a word it would take as an option; titles that start with - go after --
- The review split is kebbie's own view beside the diff: the task's steps, or its commits, as tabs with their marks, the selected one's commit message and its files on top, and nvim below showing the file you pick at once; ← → pick a step, ↑ ↓ a file, a accepts the step and moves on to the next one waiting, r marks a file read, / filters, m hides the message, and a click picks a step or a file
- tab moves between the steps, the files and the diff, and back; a review 120 columns wide or more puts the diff beside the view instead of under it
- The review split needs only nvim, not fzf
- The board's To review group is In review; board.groups takes "in-review", and "to-review" still works
- Messages to the lead, kebbie's notices and desktop notifications name a task the way the board does, by its title with its name
- kebbie doctor lists worktrees and open tasks left from kelpie, and kebbie doctor --fix installs kebbie's push block where kelpie's was

### Fixed
- Messages from kebbie show in Claude Code as one collapsed line from @kebbie that ctrl+o expands, instead of in full with Claude Code's peer warning
- y in the review split's diff copies to the clipboard, also over SSH from a machine with no clipboard tool, through the terminal
- c in the review split's diff comments on lines selected with a drag or V
- A question answered in your own words that asks the task something gets an answer from the task, as a new question on the board, before it writes code
- prefix+space in a lead Space whose board is not running starts the board, instead of leaving the lead
- Moving over from kelpie points herdr's sidebar sync and key descriptions at kebbie too, and reloads herdr, so the sidebar runs and prefix+space works without restarting herdr
- Running the install command again no longer moves kelpie over a second time because of the log the old keeper leaves as it exits
- Moving over from a kelpie linked from a local checkout unlinks it from herdr and leaves the checkout, so the next install no longer moves kelpie over again
- A CI run that GitHub cancelled, because a deploy window refused it or a newer push superseded it, is no failure: the task stays under CI running, says which workflow waits for the next run on main, and closes once a later run on main that holds the pushed commit passes
- A worktree whose push block kelpie installed takes kebbie's own push again: kebbie update and kebbie doctor --fix install kebbie's block there, and a task that starts or restarts in it gets it too
- A task whose hooks still run kelpie's removed binary restarts once it is idle, with settings that run kebbie, so its test runs and its vault gate work again; kebbie restart says what it restarted each session for
- A task's git push to a local path or a file:// URL, such as a repo a test makes, goes through; every network URL stays blocked
- A task's git push to a host:path URL such as git@github.com:acme/app.git fails at once, as every other network URL does, instead of starting ssh to a host named kebbie-blocks-push; kebbie update renews the block in every pooled worktree

## 0.28.1

### What's new
- The install command says what it installs, from which tag and commit, and what it changes, then asks once; kelpie is only moved over after that yes, and herdr's own listing shows only when the install fails
- kebbie setup has no hub step and never touches the hub; once it is done, a machine that is not connected gets a box offering the board on your phone: h connects now, any other key closes, and kebbie hub connect or H on the board connects later
- kebbie setup fixes the kebbie doctor warnings it can as rows of its review, shown before ⏎ like every other: kebbie's statusLine in Claude Code, the keeper and kebbie's herdr plugin; it ends with the doctor's count and next steps of a command and why each, and a warning that needs your choice is one line to kebbie doctor
- kebbie setup's review says each line in a few words, such as ~/.local/bin/kebbie or prefix+f added, herdr reloaded

### Fixed
- The hub's QR code is drawn in half blocks, which every font draws, in terminals other than Ghostty, kitty and WezTerm, and in octant blocks in Ghostty over SSH too; KEBBIE_QR=halfblock or octant picks one

## 0.28.0

### What's new
- Coming from kelpie: the install command moves your settings, state and herdr keys over to kebbie and removes kelpie's plugins from herdr and Claude Code
- kebbie setup is a guide of eight steps, one box each with the steps on the left: this machine's name, the agents (Claude Code, and Codex on or off), herdr's keys, the backlog's sources, the vault (a folder or an S3 bucket, configured for you with nothing to paste), the hub (straight to its connect card) and error signals, then a list of everything it will write or run, applied on ⏎ line by line, and next steps: what is still yours to do, each with its command or key, such as codex login, kebbie hub connect or a kebbie doctor fix; nothing is written before that, s skips a step, ← goes back and esc quits
- Run again, kebbie setup starts from this machine's values and marks what is already in place; kebbie setup --yes takes the defaults without asking, for scripts, and kebbie setup vault and kebbie setup codex open the guide at their step
- A herdr key you already use for something else is named in the setup guide, with a free key offered for kebbie's
- The board has a Settings page, on , or with kebbie config edit: every key kebbie config prints, by section, with its value, the layer it comes from and what it does, changed in place in config.toml or the repo's config with its comments and order kept, each write checked by kebbie doctor; a key from a linked team file asks whether to change the team's file or override it here
- On the Settings page, task.rules_skill and worktree.setup_skill offer a copy of kebbie's skill in ~/.claude/skills to edit as your own
- task.rules_skill and worktree.setup_skill take a Markdown file by its absolute or ~ path as well as a skill's name: the task gets the file's text in its role, and the reviewer and the rules check in their brief
- kebbie config and the configuration reference say plainly that restart.auto restarts only the sessions running older code than is installed, after an update or a new Claude Code, and that restart.idle_minutes is how long such a session waits to be idle and out of focus first
- kebbie setup adds three herdr keys, prefix+space, prefix+f and prefix+t; updating is U on the board, so prefix+u is no longer added
- Install with one command, curl -fsSL https://kebbie.dev/install.sh | sh: it checks what kebbie needs, installs the latest release in herdr and runs setup, with no herdr restart and no path to paste
- Connecting a machine from its link hands it your account's key: the machine checks the key against the link's secret, pins it, and seals every board to your account alone, so any phone or browser you sign in on with your passkey reads it, with nothing to add on the machine; kebbie hub status shows the account and its key's two fingerprints
- A machine approved by its typed code, or joined with a join code, posts no board, and the card, the H window, kebbie hub status and kebbie doctor say to connect it from the link: kebbie hub leave, then kebbie hub connect
- The board goes out at least once a minute while nothing changes, well within the day the hub keeps it, so an idle machine stays awake on the web and one really asleep shows as asleep
- Notifications through the hub are not available in this version: the phone shows the board when you open it, and notify.command still tells you on the machine when a task needs you
- Connecting a hub opens the browser only when you press ⏎, so you can read the card first
- The hub's QR code for connecting is drawn small in the terminal's own colours, in octant blocks in Ghostty, kitty and WezTerm, which the iPhone camera reads as a code and not as text, and in braille elsewhere; KEBBIE_QR=octant or braille picks one, and in a narrow pane the code sits under the text
- KEBBIE_HUB_DEV=1 points kebbie hub, the H window and the connect card at a dev hub on localhost:8470 (KEBBIE_HUB_DEV_URL for another address), with its own identity in hub-dev/, and says dev hub wherever it shows the hub; the real hub connection stays as it is, and the keeper posts the board to both
- kebbie setup and kebbie hub connect pick the hub with ↑ ↓ and ⏎; k and o still choose at once
- The hub window shows this machine on its hub with when the board last went out and where to sign in to see it; n renames this machine and l leaves the hub
- Connecting a hub shows a card: a QR code, the code to check on the page, the time left, and c to copy the link
- kebbie setup offers the hub as an optional last step, and asks which hub before the machine's name
- Signing in on the kebbie hub no longer asks for an account name
- kebbie speaks of the bugs in kebbie you ran into, plainly, on the board and in its messages
- The update shows each step as it goes, with a bar while kebbie builds or downloads, and ends with what's new and what restarts
- The line about the lead restarting on the new kebbie is short and always fits the board
- One bar shows the whole update, from fetching to the restarts, weighed by how long each step took last time, and the summary comes once it is full
- On the main channel kebbie calls itself kebbie main, in kebbie --version, kebbie doctor and the update window, and the board's ? window ends with which kebbie runs
- On the main channel the board's update notice counts the new commits, ↑ kebbie main, 3 new, and the update window names the commits it went from and to
- L on the board asks whether to start the lead fresh, on a new conversation from its role and the vault, or resume it; kebbie lead --restart --fresh starts it fresh, and the board says when the lead's context is nearly full

### Fixed
- The hub's QR code draws its light modules and quiet zone in the text colour and leaves the dark ones to the background, so on a dark theme it reads dark on light as a QR code should
- L on the board restarts the lead whenever you press it, and its result goes by like the board's other results
- prefix+space brings the board back in the lead Space after you quit it or closed its pane, and creates the lead Space with its board when there is none, never a second board
- The lead and other sessions restart onto each new kebbie again; a restart for an earlier update no longer holds them back
- The update's summary lists what was written since the kebbie you had, names only the bugs this update fixed, keeps its restarts up to date while open, and the board restarts itself onto the new kebbie when you close it
