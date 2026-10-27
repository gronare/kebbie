# Changes

What each kebbie release brings, one line per change, as you would tell someone who uses kebbie. The board's update window shows the entries of the update it installed; scripts/release.sh moves the Unreleased entries under the new version.

## Unreleased

### What's new
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
