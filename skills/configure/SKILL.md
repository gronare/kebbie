---
name: configure
description: Set kelpie up on this machine and change its settings, in config.toml or a repo's own repos/<repo>.toml, including a team's shared setup linked in from a team repo. Use when the user asks to configure kelpie or change a kelpie setting, when another kelpie skill says to write a kelpie key, or when a team repo describes a kelpie setup to adopt.
---

# Configure kelpie

`kelpie` is `${KELPIE_BIN:-$HOME/.local/share/kelpie/current/target/release/kelpie}`. The main checkout is the parent of `git rev-parse --path-format=absolute --git-common-dir`.

kelpie reads its settings from two files on this machine:

- **config.toml**, the `machine config:` line of `kelpie config`: settings for every repo.
- **The repo config**, the `repo config:` line of `kelpie config --repo <main checkout>`: one file per repo in kelpie's repos folder, named after the main checkout's folder (`app.toml` for `~/projects/app`). It holds `[worktree]`, `[task]`, `[review]` and the repo's `[[actions]]`, and wins over config.toml key by key.

Every write in this skill lands in one of those two files, or in a team repo they link into. The work repo stays as it is.

**The key list** is `kelpie config --repo <main checkout>`: every key with its value in effect, the layer it comes from, what it does, and which keys are config.toml only. It is the source for key names and meanings; docs/reference/configuration.md adds examples.

## Change a setting

1. Run `kelpie config --repo <main checkout>` and find the key. Done when you know the key, its value in effect and the file it comes from.
2. Pick the file. A key marked config.toml only, or one the user wants for every repo, goes in config.toml; a value for one repo goes in its repo config. When `kelpie config` shows the file as a link (`<file> -> <target>`), the target belongs to a team repo: edit the target when the user wants the team's value changed, and leave the commit there to the user; when the user wants a value of their own, ask before replacing the link with a file. Done when you have one file path and the user's intent fits it.
3. Edit the TOML, keeping every other line. Create the file, and the repos folder with `mkdir -p`, when missing. Done when the key holds the new value.
4. Verify with `kelpie doctor`, which checks config.toml and every file in the repos folder. Done when the line for each file you touched reads `ok` and ends `parses`, and `kelpie config --repo <main checkout>` shows the new value with the layer you wrote. An error names the file and the key: fix it and run both again.

## Adopt a team's setup

A team keeps its kelpie files in a team repo, and each developer links them in, so a team change reaches everyone on their next pull.

1. Clone the team repo next to the user's other checkouts, or `git -C <team repo> pull` when it is already here. Ask the user for its URL when the request does not name it. Done when the team repo is current on this machine.
2. Find the file that describes its kelpie setup: `grep -ril kelpie <team repo>` over its READMEs, AGENTS.md, CLAUDE.md and skills. Done when you have every pair of `file in the team repo → kelpie path` it lists, and anything else it asks for. When no file describes one, ask the user which files to link.
3. Link each pair with `ln -s <absolute path in the team repo> <kelpie path>`, after `mkdir -p` on the kelpie path's folder. A kelpie path already linked to that same target is done; any other file there, show it next to the team's and ask the user which to keep. Then do the rest the description asks for (a plugin to install, a key to add to config.toml). Done when every pair is a link.
4. Verify with `kelpie doctor`. Done when every linked repo config reads `ok <kelpie path> -> <team path> (found) parses`; a `points at nothing` line is a link to a path that is missing in the team repo.

## Describe a setup in a team repo

When the user wants their own setup shared, the team repo gets one short file that step 2 above finds, such as `kelpie/README.md`, listing each file and where it links:

```markdown
# kelpie setup

Link each file into kelpie's config folder with `ln -s`, or ask Claude to adopt it with `/kelpie:configure`.

| File here | Link at |
| --- | --- |
| `kelpie/repos/app.toml` | `~/.config/kelpie/repos/app.toml` |
```

A repo config's name is the folder each developer clones that repo into, so the file says which folder name it expects when that differs from the repo's name. Done when the file lists every shared kelpie file and `kelpie doctor` reads `ok` for each link on this machine.
