# Updates and stale sessions

![U on the board: kebbie updates and restarts the idle Claude sessions](../assets/clips/update-and-restarts.gif)

kebbie updates itself, its Claude Code plugin and the vault plugin with one command, and then moves the running Claude sessions onto the new code without losing their conversations. This page explains how.

## One install root

herdr installs kebbie into a folder of its choosing; herdr's startup links `~/.local/share/kebbie/current` to it. The binary on your `PATH`, the Claude Code plugin's marketplace and the hooks all point through that link. When `kebbie update` checks out new code there and installs its binary, every part moves together.

## Prebuilt binaries

herdr's install and `kebbie update` both put the binary in place with `scripts/build.sh`, in this order:

1. A `target/release/kebbie` that is newer than every source file is kept.
2. When the checkout is exactly at the `vX.Y.Z` tag of its version and the machine is not on the `main` channel, the script downloads `kebbie-<version>-<target>.tar.gz` from that tag's published GitHub Release, checks it against the release's `SHA256SUMS`, and renames it into place. Releases are built for macOS on Apple silicon and Intel, and for Linux on x86_64 and arm64 (glibc).
3. Otherwise it runs `cargo build --release`.

A release tag is never built from source: when its download fails, the script stops with `kebbie <version> is not published yet; try again in a while` and leaves the binary it has, whether `kebbie update` or herdr's own install (`herdr plugin install --ref vX.Y.Z`) ran it. The next run tries the download again. A release tag with no binary to fetch, on another platform, from an origin that is not on GitHub or without `curl`, stops too and says why. On the release channel `kebbie update` builds nothing at all. Only the `main` channel, `kebbie update --from` and a checkout that is not at a release tag build from source. herdr's install runs the script without kebbie's channel, so the script reads it from the `[update]` table of the machine config (`channel = "main"` on its own line; a dotted `update.channel` key or an inline table is not read there).

Before it builds, the script checks the Rust toolchain. With rustup, the first `cargo` installs the version `rust-toolchain.toml` pins. Without rustup, a `rustc` older than the `rust-version` in `Cargo.toml` stops the build with `Rust 1.89 is too old to build kebbie (it needs 1.90): install rustup, or switch to the release channel`, before `cargo` starts. `kebbie doctor` shows on its `plugin binary` line whether the binary was downloaded, and for which tag, or built, and its `cargo` line says whether this machine needs Rust.

## Channels

Each machine follows one channel:

- **`release`**, the default, installs the highest `vX.Y.Z` tag on origin whose GitHub Release is published with a binary for this machine. The install root is checked out detached at that tag. A tag whose release is still a draft, or whose binary GitHub does not serve yet, is never offered: `kebbie update` stays on the published release and says `kebbie <version> is not published yet; try again in a while`. Only tags of exactly `v` and three numbers count, so `v1.4` and `v1.4.0-rc1` are never offered.
- **`main`** follows origin's default branch as it is pushed.

Because the release channel checks out tags in the install root, it refuses to touch an install root with uncommitted changes or one that sits on a branch. Develop in another checkout and try its commits with `kebbie update --from <path>`.

The keeper checks for updates every 6 hours, and the board shows `↑ kebbie <version> · U updates` when the channel has something newer. On a release machine the notice names the new tag, never what main carries.

The installed version is the one the binary itself reports, with the commit it was built from, not the version in the checkout's files. A checkout that moved ahead without a rebuild shows as `0.24.0 installed (e70d5d5), 0.25.0 in the checkout: rebuild`, the board offers the update, and `U` rebuilds it.

## Overrides

`kebbie update --from <branch or path>` installs a branch on origin or another checkout's commits, and holds it: a plain `kebbie update` fetches the same branch or checkout again. `kebbie update --check`, `kebbie doctor` and the board say `running <branch or path> @ <sha>, not the channel` until `kebbie update --stable` returns you to the channel. Only commits are installed, so a checkout with uncommitted changes is refused.

## The Claude Code plugin

Claude Code runs a cached copy of each plugin. When the kebbie marketplace was added from `~/.local/share/kebbie/current`, `kebbie update` refreshes that copy from whatever it just checked out.

A marketplace added from GitHub follows GitHub's default branch instead. `kebbie update` updates it only on the `main` channel without an override; otherwise it leaves it and prints the commands that move it to the install root. `kebbie doctor` warns about it too.

## Stale sessions

A running Claude session keeps the code it started with. kebbie calls it stale when it runs older code than it would load if it started now:

- the `claude` binary on `PATH` changed after it started,
- a plugin was updated after it started,
- your MCP server config changed after it started, or
- `kebbie update` ran after it started, or
- it is a task's Claude running without the role and settings kebbie gives a task, or its role file is gone. Such a session has none of kebbie's hooks and never reports, so the board would keep it under Needs you.

A restart is `claude --resume <session id>` in the same pane, with the process's own flags such as `--model`, `--permission-mode` and `--add-dir`. The conversation stays, and the new process loads the new Claude Code, plugins, MCP servers and hooks. A task's Claude gets its role and settings written anew, as `kebbie start` writes them, in place of the ones it ran with, from the profile it started with. When that profile no longer loads, the task restarts as the built-in `task` and the lead is told which file failed and why. The lead is restarted through `kebbie lead --restart`, which also rewrites its role.

Going back to an older kebbie, it reads a task's session record, now JSON, as its session id: it finds no conversation by it and resumes the task's newest one, and the task loses its recorded mode and profile until its next start.

## What an update shows

`U` on the board opens the update's window, and `kebbie update` in a terminal prints the same steps:

```
╭ Updating kebbie ───────────────────────────────────────────╮
│ ████████████████░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░  31%  │
│                                                            │
│ ✓ Fetched main (3 new commits)                             │
│ ▸ Building kebbie  ████████████░░░░░░░░  212/348 crates 1:42│
│   Claude Code and vault plugins                            │
│   Restart sessions                                         │
│                                                            │
│ esc hides this; the update keeps going                     │
╰────────────────────────────────────────────────────────────╯
```

The bar on top is the whole update: fetching, getting the binary, the plugins and the restarts of the sessions that can restart now. It reaches the end only once those restarts are done; a session that restarts when it is idle does not hold it. Each step weighs what it took on this machine's channel the last time, kept in `update-durations.json` in the state directory and moved halfway to each new measure; a first update guesses that a `main` build takes most of the time and that on the release channel the plugins and restarts do. Within a step the bar follows the crates built, the megabytes downloaded, the plugins updated and the sessions gone through. The bar starts slow and speeds up towards the end, as people feel such a bar is the shortest wait (Harrison et al., "Rethinking the progress bar", 2007), and it never moves back.

The step running shows how long it has run. While kebbie builds, the bar counts the crates cargo has finished against the crates the build needs, which kebbie reads from `cargo metadata`; until it has that count it names the crate compiling. On the release channel the bar counts the megabytes of the release binary downloaded. The update runs on in the background: `esc` hides the window, the status line shows `⇣ updating kebbie · U` meanwhile, and `U` opens the window again. The board does not restart on a new binary while its update runs. In a terminal the bar and the steps are redrawn in place; piped, each step prints one plain line. The summary shows once the bar is full.

When it is done, the window says what the update brought:

```
╭ kebbie 0.27.0 ─────────────────────────────────────────────╮
│ ✓ Updated                                                  │
│                                                            │
│ What's new                                                 │
│  • The hub window shows your devices by name, and r removes│
│    one                                                     │
│ Fixed                                                      │
│  • A bug you ran into: update stopped on a newer config key│
│                                                            │
│ Restarting                                                 │
│  ✓ The lead and 3 sessions are on the new kebbie           │
│  1 session restarts when it's idle                         │
│                                                            │
│ d details · any other key closes                           │
╰────────────────────────────────────────────────────────────╯
```

**What's new** and **Fixed** are the handwritten notes in kebbie's `CHANGES.md`: on the release channel the sections of the versions after the one you had, up to the one installed; on the `main` channel the entries added since the commit you had. With nothing written it says `No notes for this update`; it never lists commits. On the `main` channel the commit you had is the one your kebbie binary was built from, so a checkout that already held the new commits before the update still shows what they wrote. **Fixed** also names the bugs in kebbie you ran into that the recheck closed on the build this update installed, as they come in; one an earlier update closed, or one already shown, stays out. **Restarting** sums the sessions up and follows them while the window is open, so a session that restarts when it's idle moves to `on the new kebbie` once it has. The board's window names no board: the board restarts itself onto the new kebbie when you close the summary. `kebbie update` in a terminal says on one line that open boards restart on the new kebbie when you next open them. `d` shows the details: each component's version before and after with its commit, the lead's kebbie, each session's line, the plugins, the board and the keeper. An update that stops names the step it stopped at and why.

## When restarts happen

With `restart.auto` on, the default, `kebbie update` restarts idle sessions at once, and the keeper restarts each other stale session once it has been idle and out of focus for `restart.idle_minutes`; the lead waits one minute instead. With `[keeper] enabled = false`, the full board does this every minute; the `prefix+t` popup does not. The status line shows `↻ N stale` while sessions wait. With `restart.auto = false`, nothing restarts on its own: `kebbie restart`, `kebbie lead --restart` and the board's `L` do it when you choose.

A restart on its own never interrupts work:

- A session mid-turn, or the pane you are in, is never restarted on its own, as what you type would land in a pane being ended.
- A session with background work running, such as a background shell, an agent or a monitor, is not cut off. The board asks you once, as a question.
- A session with text in its input box that you have not sent, or whose box kebbie cannot read, waits until the box is empty, so a restart never loses a draft. A restart you ask for refuses typed text too, but goes ahead when the box cannot be read.
- A session kebbie cannot restart safely is skipped with the reason: one whose session id herdr does not know, whose command line cannot be read or has an option kebbie does not know, whose Claude runs in another folder than its pane, or whose conversation file is missing.

`kebbie update` sums the sessions up in a line or two, such as `✓ The lead and 3 sessions are on the new kebbie` and `1 session restarts when it's idle`, and names only what needs you: a session waiting for your answer, one a restart did not move, or a restart that failed. Its details then list one line per session: `restarted`, `restarts when idle`, `you are in it: restarts when you leave it`, `its input box holds text or cannot be read: restarts once it is empty`, `waiting for your answer: background work running`, or `skipped: <reason>`.

## Restarting the lead after an update

The lead is usually the session you are talking to, so an update often lands while it works or while you are in its pane. kebbie records which kebbie the lead started on, and while the lead counts as stale the Work page shows one line under the status line, such as `↻ The lead restarts on the new kebbie when it's idle · L restarts it now`, or `when you leave it` while you are in its pane. It is never cut: on a narrow board it reads `↻ Lead restarts when idle · L now`. Which kebbie the lead runs and which is installed, with their commits and dates, are in the update's details (`d` in its window).

With `restart.auto` on, the keeper restarts the lead the same way it restarts any session, but a minute after it is idle and you are out of its pane, such as on the board beside it, rather than after `restart.idle_minutes`. The restart is `kebbie lead --restart`, so the conversation is resumed under the new role, and the lead says in one line that it will be restarted; you never need another pane for it. With `restart.auto = false` the line says `↻ The lead runs the old kebbie until you restart it · L restarts it`, and the lead says so too.

`L` on the board, or a click on the line, restarts the lead at once, also while you are in its pane. It is refused with the plain reason while the lead works, waits on a question, holds text you have not sent or has background work running, and you can press it again later. The keeper and `L` never restart the lead together: each takes the lead's lock first, and the second is refused. Once the lead runs the installed kebbie, the line goes. A refusal costs nothing: the line stays as it is and the keeper tries again on its next round, a minute later. A restart that fails after it ended or started the lead's Claude is told in the messages, the line says `↻ The lead's restart on the new kebbie failed · L tries again`, and the keeper waits 30 minutes before it tries again; `L` does not wait.

## Bugs you ran into in kebbie

An update can fix the bugs in kebbie you ran into and the wishes you saved for it. After `kebbie update`, the keeper rechecks each open one ([kebbie issue](../how-to/report-a-problem-with-kebbie.md#after-an-update)) filed on older code in the background: a read-only Claude reads the issue, the commits since and the new source, and kebbie closes the issues a commit plainly fixes. You are not asked anything; the board shows one line naming what the update fixed, such as `✓ Fixed in kebbie 0.27.0: 1 bug you ran into`.

## The keeper

The keeper is the background `kebbie keeper` process that runs the restarts, the update check, the delivery of held messages and typed prompts, and the relay of a task's Claude's own prompts to the lead while no board is open. See [How kebbie fits together](architecture.md#the-keeper).

It starts in three places, and each starts it only when none runs:

- herdr's startup hook, when the herdr server starts,
- each board, when it opens and every minute after,
- `kebbie update`, at its end.

`kebbie update` stops the keeper before it restarts sessions, so the two never restart one together, and starts a keeper on the new binary once it is done. A keeper also watches its own binary: once the file it runs from is replaced and holds still for two checks, as after a rebuild or when the `current` link moves to a new install, it exits and starts the new binary in its place.

`kebbie keeper --status` shows whether one runs, its pid, binary and uptime, and each routine's last run; `kebbie keeper --stop` stops it after the routine under way.

## After herdr restarts

herdr saves its Spaces and panes and restores them when its server starts again. A pane that ran Claude comes back with that conversation resumed, unless herdr's `[session] resume_agents_on_restore` is off, so tasks and the lead keep their Space and their Claude. herdr resumes each Claude once a client is attached to show it, so a task can show `no Claude` for a moment after the restart.

Any other program comes back as a bare shell, the board in the lead Space too. herdr's startup hook therefore runs `kebbie lead --revive-board`, which returns at once and leaves a background kebbie to wait for the lead Space, looking every 2 seconds for up to a minute. When the lead Space's board pane sits at a shell prompt, it starts the board there. A board already running, or a program of your own in that pane, is left alone, and the lead's chat is left to herdr. Without a lead Space it does nothing. It runs whether or not the keeper is enabled, and only when the herdr server starts, never when the keeper restarts itself after an update, so a board you quit on purpose stays quit until herdr starts again or you go to the lead. What it did is in `lead.log` in the state directory.

## Related

- [Switch update channel](../how-to/switch-update-channel.md)
- [Try unreleased code](../how-to/try-unreleased-code.md)
- [Turn off automatic restarts](../how-to/turn-off-automatic-restarts.md)
