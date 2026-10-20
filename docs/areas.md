# Areas

The Areas page gives one row per system you keep notes on, with its repo. Press `2` on the board to open it and `1` to go back to Work. `kelpie areas` prints the same on the command line.

Areas needs the [vault](vault.md): it lists the vault's system notes (`Areas/*.md`).

## What it shows

Areas are grouped into **needs you** (main ahead of origin, commits since the last release, or a task waiting on you), **active** and **quiet**. Each shows its running tasks, open notes and backlog, main against origin, the last tag and commits since, and the last log line. Areas without a repo on this machine still list.

The repo is the system note's `path`, else where the vault's hooks saw it, else a checkout named after the area. kelpie reads [notes-vault-mcp](https://github.com/gronare/notes-vault-mcp)'s local index, so a vault on S3 needs no credentials here. It is as fresh as the last session that used the vault.

## Actions

`⏎` lists an area's actions, and `←` `→` move into the detail to pick one. Each has a letter that also works directly.

| Key | Action |
| --- | --- |
| `g` | Open the main checkout's Space. |
| `s` | Start a task in this repo, told the area's notes. |
| `a` | Let the lead push every task here whose reviewer passed its current code, or take that back. |
| `b` | Ask the lead to refresh this area's backlog. |
| `l` | Show the area's vault log, newest first. |
| `u` | Push main, shown only when main is ahead of origin. Shows the commits and pushes as you on `y`. |

## Your own actions

A repo gets its own actions in its repo config, `~/.config/kelpie/repos/<repo>.toml`, one `[[actions]]` table each:

```toml
[[actions]]
key = "R"                     # one character, not one of g s b l u a j k q ?
title = "Release"
about = "bump, commit, tag, push"
ask = "version"               # asked in turn; a version is prefilled with the next patch
bump = ["Cargo.toml", { file = "Cargo.lock", after = 'name = "billing"' }]
run = ["cargo build"]         # shell commands in the main checkout, after the bump
commit = "{version}"
tag = "v{version}"
push = true                   # asks again, then pushes main and the tag as you
then = ["web:bump-client"]    # repo:action offered once this is done, never run on its own

[[actions]]
id = "bump-client"            # defaults to the title, slugged
key = "P"
title = "Bump the client"
ask = ["package", "version"]
task = "Bump {package} to {version}; commit."   # starts a kelpie task with a Claude instead
```

| Key | What it does |
| --- | --- |
| `id` | The action's name for `then`. Defaults to the title, slugged. |
| `key` | One character. Not one of `g s b l u a j k q ?`. |
| `title`, `about` | What the board shows. |
| `ask` | A value or list of values asked in turn, used as `{name}` below. A `version` is prefilled with the next patch. |
| `bump` | Files whose version lines change. `after` limits a file to the first such line after a marker. |
| `run` | Shell commands run in the main checkout after the bump. |
| `commit`, `tag` | The commit message and tag to create. |
| `push` | Push main and the tag as you, after asking once more. |
| `task` | Start a kelpie task with this prompt instead of running a script. |
| `then` | `repo:action` pairs offered once this one is done. |

A script action runs in the main checkout, which must be on the default branch, clean and not behind origin. The window shows each step first, including the exact lines a bump changes, and nothing changes until you press `y`. A bump touches only lines that mention a version and hold the current one. Paths stay inside the repo.
