# Changes

What each kebbie release brings, one line per change, as you would tell someone who uses kebbie. The board's update window shows the entries of the update it installed; scripts/release.sh moves the Unreleased entries under the new version.

## Unreleased

### What's new
- Connecting a hub opens the browser only when you press ⏎, so you can read the card first
- The hub's QR codes, for connecting and for adding a phone, are drawn small in the terminal's own colours, in octant blocks in Ghostty, kitty and WezTerm, which the iPhone camera reads as a code and not as text, and in braille elsewhere; KEBBIE_QR=octant or braille picks one, and in a narrow pane the code sits under the text
- kebbie setup and kebbie hub connect pick the hub with ↑ ↓ and ⏎; k and o still choose at once
- The hub window shows this machine on its hub with when the board last went out and where to sign in to see it; n renames this machine and l leaves the hub
- Connecting a hub shows a card: a QR code, the code to check on the page, the time left, and c to copy the link
- kebbie setup offers the hub as an optional last step, and asks which hub before the machine's name
- kebbie hub remove-device removes a device from the command line
- Signing in on the kebbie hub no longer asks for an account name
- kebbie speaks of the bugs in kebbie you ran into, plainly, on the board and in its messages
- The update shows each step as it goes, with a bar while kebbie builds or downloads, and ends with what's new and what restarts
- The line about the lead restarting on the new kebbie is short and always fits the board
- One bar shows the whole update, from fetching to the restarts, weighed by how long each step took last time, and the summary comes once it is full
- On the main channel kebbie calls itself kebbie main, in kebbie --version, kebbie doctor and the update window, and the board's ? window ends with which kebbie runs
- On the main channel the board's update notice counts the new commits, ↑ kebbie main, 3 new, and the update window names the commits it went from and to
- L on the board asks whether to start the lead fresh, on a new conversation from its role and the vault, or resume it; kebbie lead --restart --fresh starts it fresh, and the board says when the lead's context is nearly full

### Fixed
- L on the board restarts the lead whenever you press it, and its result goes by like the board's other results
- prefix+space brings the board back in the lead Space after you quit it or closed its pane, and creates the lead Space with its board when there is none, never a second board
- The lead and other sessions restart onto each new kebbie again; a restart for an earlier update no longer holds them back
- The update's summary lists what was written since the kebbie you had, names only the bugs this update fixed, keeps its restarts up to date while open, and the board restarts itself onto the new kebbie when you close it
