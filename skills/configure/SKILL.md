---
name: configure
description: Set kebbie up on this machine and change its settings, in config.toml or a repo's own repos/<repo>.toml, including a team's shared setup linked in from a team repo. Use when the user asks to configure kebbie or change a kebbie setting, when another kebbie skill says to write a kebbie key, or when a team repo describes a kebbie setup to adopt.
---

# Configure kebbie

`kebbie` is `${KEBBIE_BIN:-$HOME/.local/share/kebbie/current/target/release/kebbie}`. The main checkout is the parent of `git rev-parse --path-format=absolute --git-common-dir`.

kebbie reads its settings from two files on this machine:

- **config.toml**, the `machine config:` line of `kebbie config`: settings for every repo.
- **The repo config**, the `repo config:` line of `kebbie config --repo <main checkout>`: one file per repo in kebbie's repos folder, named after the main checkout's folder (`app.toml` for `~/projects/app`). It holds `[worktree]`, `[task]`, `[review]` and the repo's `[[actions]]`, and wins over config.toml key by key.

Every write in this skill lands in one of those two files, or in a team repo they link into. The work repo stays as it is.

**The key list** is `kebbie config --repo <main checkout>`: every key with its value in effect, the layer it comes from, what it does, and which keys are config.toml only. It is the source for key names and meanings; docs/reference/configuration.md adds examples.

## Set kebbie up on this machine

1. Name the machine first. When `kebbie config` shows `machine.name` with the default layer, it is unset; skip this step when it is set. Ask the user for a name, saying it is how their phone shows this machine and how their other machines working on the same vault see it, and offer the host's short name as the default. Give each machine its own, such as `mac` or `devbox`; `localhost` is refused. Write `[machine] name = "<name>"` in config.toml as in step 3 of Change a setting. Done when `kebbie config` shows `machine.name` from the machine config.
2. Run `kebbie doctor` and work through each `warn` and `fail` with its `fix:` line, changing settings as below. Done when nothing says `fail`.

## Tell the user what goes to kebbie's maintainers

Do this right after naming the machine when setting kebbie up, and whenever `kebbie config` shows `report.signals` at `(default)`:

1. Tell the user, in these words or close: kebbie sends its maintainers error signals when kebbie itself fails (a panic, and some internal errors), at most once a day. A signal holds only kebbie's version and commit, the OS and CPU architecture, the component (a fixed name from kebbie's code, such as `keeper.push`), the error kind (`panic`, `internal-error` or `corrupt-state`) and a count; never an error message, a path, or a repo, task or host name. Each send is a line under messages on the board. Bug reports with text are shown on the board and sent only on their `y` (`report.text = "ask"`).
2. Ask whether to keep signals on. On no, write `signals = false` under `[report]` in config.toml. For bug reports you may write `text = "never"`; never write `text = "always"` yourself: when the user wants reports sent without asking, tell them to run `kebbie issue report --always` in their own terminal and to set `text = "always"`. Done when `kebbie config` shows `report.signals` and `report.text` as the user chose, and `kebbie doctor`'s `maintainers` line reads `ok`.

## Keep the lead from proposing kebbie issues

When the user wants the lead to stop proposing kebbie issues, write `propose = false` under `[issues]` in config.toml; `propose = true`, or no key, has it propose one at most per turn and save it on the user's yes. Tell the user it takes effect when the lead next restarts, with `L` on the board. Done when `kebbie config` shows `issues.propose` as the user chose.

## Change a setting

1. Run `kebbie config --repo <main checkout>` and find the key. Done when you know the key, its value in effect and the file it comes from.
2. Pick the file. A key marked config.toml only, or one the user wants for every repo, goes in config.toml; a value for one repo goes in its repo config. When `kebbie config` shows the file as a link (`<file> -> <target>`), the target belongs to a team repo: edit the target when the user wants the team's value changed, and leave the commit there to the user; when the user wants a value of their own, ask before replacing the link with a file. Done when you have one file path and the user's intent fits it.
3. Edit the TOML, keeping every other line. Create the file, and the repos folder with `mkdir -p`, when missing. Done when the key holds the new value.
4. Verify with `kebbie doctor`, which checks config.toml and every file in the repos folder. Done when the line for each file you touched reads `ok` and ends `parses`, and `kebbie config --repo <main checkout>` shows the new value with the layer you wrote. An error names the file and the key: fix it and run both again.

## Adopt a team's setup

A team keeps its kebbie files in a team repo, and each developer links them in, so a team change reaches everyone on their next pull.

1. Clone the team repo next to the user's other checkouts, or `git -C <team repo> pull` when it is already here. Ask the user for its URL when the request does not name it. Done when the team repo is current on this machine.
2. Find the file that describes its kebbie setup: `grep -ril kebbie <team repo>` over its READMEs, AGENTS.md, CLAUDE.md and skills. Done when you have every pair of `file in the team repo → kebbie path` it lists, and anything else it asks for. When no file describes one, ask the user which files to link.
3. Link each pair with `ln -s <absolute path in the team repo> <kebbie path>`, after `mkdir -p` on the kebbie path's folder. A kebbie path already linked to that same target is done; any other file there, show it next to the team's and ask the user which to keep. Then do the rest the description asks for (a plugin to install, a key to add to config.toml). Done when every pair is a link.
4. Verify with `kebbie doctor`. Done when every linked repo config reads `ok <kebbie path> -> <team path> (found) parses`; a `points at nothing` line is a link to a path that is missing in the team repo.

## Describe a setup in a team repo

When the user wants their own setup shared, the team repo gets one short file that step 2 above finds, such as `kebbie/README.md`, listing each file and where it links:

```markdown
# kebbie setup

Link each file into kebbie's config folder with `ln -s`, or ask Claude to adopt it with `/kebbie:configure`.

| File here | Link at |
| --- | --- |
| `kebbie/repos/app.toml` | `~/.config/kebbie/repos/app.toml` |
```

A repo config's name is the folder each developer clones that repo into, so the file says which folder name it expects when that differs from the repo's name. Done when the file lists every shared kebbie file and `kebbie doctor` reads `ok` for each link on this machine.
