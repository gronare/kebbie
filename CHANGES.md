# Changes

What each kebbie release brings, one line per change, as you would tell someone who uses kebbie. The board's update window shows the entries of the update it installed; scripts/release.sh moves the Unreleased entries under the new version.

## Unreleased

### What's new
- kelpie is now kebbie: the command, the herdr and Claude Code plugins, its folders and its environment variables all take the new name, and the hub is at https://hub.kebbie.dev
- An install from before the rename moves over in four steps, in the how-to Move from kelpie to kebbie: one herdr reinstall by hand, then kebbie moves ~/.local/state/kelpie and ~/.config/kelpie on its first run and kebbie setup switches the rest, asking before each
- The KELPIE_ environment variables you set still work under their old names for now, and kebbie says once which to rename; a notify command gets both names
- The hub window shows the devices that see your board by name, with when each was paired and last seen, and r removes one
- In the hub window, n names a device on this machine and m renames this machine
- The hub window says the board is being sent without a ticking clock, and asks you to update the hub when it is too old to list devices
- kebbie hub remove-device removes a device from the command line
- The QR code for adding a phone is drawn in true black and white, so every terminal theme shows it scannable
- Signing in on the kebbie hub no longer asks for an account name
- kebbie speaks of the bugs in kebbie you ran into, plainly, on the board and in its messages
- The update shows each step as it goes, with a bar while kebbie builds or downloads, and ends with what's new and what restarts
- The line about the lead restarting on the new kebbie is short and always fits the board
- One bar shows the whole update, from fetching to the restarts, weighed by how long each step took last time, and the summary comes once it is full

### Fixed
- The lead and other sessions restart onto each new kebbie again; a restart for an earlier update no longer holds them back
- The update's summary lists what was written since the kebbie you had, names only the bugs this update fixed, keeps its restarts up to date while open, and the board restarts itself onto the new kebbie when you close it
