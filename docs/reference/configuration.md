# Configuration

Every key has a default, so kelpie runs without a config file. `kelpie config` prints the keys with the value in effect, the layer it comes from and what it does.

## Files and layers

kelpie reads each key from the first layer that sets it:

| Layer | File | Holds |
| --- | --- | --- |
| Repo config | `~/.config/kelpie/repos/<repo>.toml` | `[worktree]`, `[task]`, `[review]` and `[[actions]]` for one repo. `<repo>` is the main checkout's folder name, so every worktree of `~/code/app` reads `repos/app.toml`. |
| Machine config | `~/.config/kelpie/config.toml` | Every key. |
| Defaults | built in | Every key. |

`$XDG_CONFIG_HOME/kelpie` replaces `~/.config/kelpie` when it is set. `KELPIE_CONFIG` and `KELPIE_REPOS_DIR` override each path; see [Environment](environment.md).

kelpie never reads settings from a file inside a repo.

Rules:

- A key marked **config.toml only** in a repo config is an error that says so.
- An unknown key in either file is an error that names the file and the key.
- Either file may be a symlink. `kelpie config` and `kelpie doctor` show each link with its target, and `kelpie doctor` reports a broken one.

## Keys

| Key | Type | Default | Where | Meaning |
| --- | --- | --- | --- | --- |
| `board.backlog.sources` | list of `"local"`, `"vault"`, `"linear"` | `["local"]` | config.toml only | Where the board's Backlog column finds work. The first source is where the lead files new ideas. `[]` turns the backlog off. See [Backlog sources](#backlog-sources). |
| `worktree.setup_skill` | string | `"kelpie:set-up-worktree"` | either | The skill a task's Claude runs first to make its worktree runnable. `plugin:skill` for a plugin's skill; a bare name for one in `~/.claude/skills` or the repo's `.claude/skills`. |
| `worktree.setup_command` | string | none | either | The repo's own command that prepares a worktree, run from inside it by the setup skill. A command naming a path applies only in a repo that has that file. Where it does not apply, and when unset, the skill runs `bin/setup-worktree` if the repo has one. |
| `worktree.dev_server` | string | none | either | The command the board's `w` runs in a server tab of the task's Space, with `PORT` set to the worktree's port. Unset, there is no `w`. |
| `worktree.dev_worker` | string | none | either | A second command, such as a job runner, that `b` runs in a worker tab, with the same `PORT`. Unset, there is no `b`. |
| `worktree.dev_url` | string | `"http://localhost:{port}"` | either | The running dev server's address, shown on the board as a link. See [URL placeholders](#url-placeholders). `""` turns the link off. |
| `worktree.server_name` | string | `"server"` | config.toml only | The name of the tab `w` starts, and of the Space's tag in herdr's sidebar while it runs. |
| `worktree.worker_name` | string | `"worker"` | config.toml only | The same for `b`'s tab. |
| `task.rules_skill` | string | `"kelpie:work-rules"` | either | The skill with the rules for how work is planned, built and committed. A task's Claude follows it, the lead checks a finished task against it, `tidy` reshapes commits by it, and the reviewer reads it. |
| `review.model` | string | none | either | The model the reviewer runs on. Unset leaves it to Claude's default. |
| `notify.command` | string | none | config.toml only | The shell command kelpie runs when something needs you. See [Notify command](#notify-command). Unset sends nothing. |
| `vault.command` | string | none | config.toml only | The notes-vault-mcp command kelpie writes task notes and log lines with, before its own arguments. Unset runs the vault plugin's pinned command from its hooks. Used only while the vault plugin is installed and enabled. |
| `update.channel` | `"release"` or `"main"` | `"release"` | config.toml only | What `kelpie update` installs: the highest `vX.Y.Z` tag on origin, or origin's default branch. |
| `restart.auto` | boolean | `true` | config.toml only | Whether the board restarts stale idle Claude sessions on its own and `kelpie update` restarts idle ones at once. `false` restarts nothing on its own; the board still marks stale sessions. |
| `restart.idle_minutes` | integer, 1 to 1440 | `10` | config.toml only | How long a stale session must be idle and out of focus before the board restarts it. |

## Backlog sources

| Source | Items |
| --- | --- |
| `"local"` | The local backlog file, `backlog.md` next to the machine config. See [Files](files.md#the-local-backlog-file). |
| `"vault"` | The vault's backlog notes. Needs the vault plugin. |
| `"linear"` | Your open Linear issues, read by the lead through a Linear MCP server. |

## URL placeholders

`worktree.dev_url` fills these in. A worktree without what the template names gets no link.

| Placeholder | Value for `wt1.app` |
| --- | --- |
| `{worktree}` | `wt1.app`, the worktree's folder |
| `{repo}` | `app` |
| `{number}` | `1`, its pool number |
| `{port}` | the worktree's port: `PORT` in its `.env`, else the port in its setup note |

## Notify command

kelpie runs `notify.command` through `sh -c`, detached, and stops it after 10 seconds. Failures go to `notify.log` in the state directory and never fail the step that sent them. A task sends at most one notification per event a minute.

The command gets:

| Variable | Value |
| --- | --- |
| `KELPIE_EVENT` | `needs-you`, `ready`, `question` or `summary` |
| `KELPIE_TASK` | The task's name |
| `KELPIE_REPO` | The task's repo |
| `KELPIE_TEXT` | One line, such as `billing@app needs you: answer which queue` |
| `$1` | The same text |

| Event | Sent when |
| --- | --- |
| `needs-you` | A task reports that it needs you. |
| `ready` | A task reports ready. |
| `question` | A task asks you a question. |
| `summary` | You or the lead run `kelpie notify "<text>"`. Tasks cannot. |

## `[[actions]]`

Repo config only. Each table adds an action to the repo's row on the board's Areas page.

| Key | Type | Meaning |
| --- | --- | --- |
| `id` | string | The action's name for `then`. Defaults to the title, slugged. |
| `key` | one character | The key that runs it. Not one of `g s b l u a j k q ?`. |
| `title` | string | What the board shows. |
| `about` | string | A line under the title. |
| `ask` | string or list | Values asked in turn, used as `{name}` in the other keys. A value named `version` is prefilled with the next patch version. |
| `bump` | list | Files whose version lines change: a path, or `{ file = "<path>", after = "<marker>" }` to change only the first version line after the marker. |
| `run` | list of strings | Shell commands run in the main checkout after the bump. |
| `commit` | string | The commit message to create. |
| `tag` | string | The tag to create. |
| `push` | boolean | Push main and the tag as you, after asking once more. |
| `task` | string | Start a kelpie task with this prompt instead of running a script. |
| `then` | list of `"repo:action"` | Actions offered once this one is done. Never run on their own. |

A script action runs only in a main checkout that is on the default branch, clean and not behind origin. The window shows every step first, including the exact lines a bump changes, and nothing changes until you press `y`. A bump touches only lines that mention a version and hold the current one. Paths stay inside the repo.

Example:

```toml
# ~/.config/kelpie/repos/billing.toml
[[actions]]
key = "R"
title = "Release"
about = "bump, commit, tag, push"
ask = "version"
bump = ["Cargo.toml", { file = "Cargo.lock", after = 'name = "billing"' }]
run = ["cargo build"]
commit = "{version}"
tag = "v{version}"
push = true
then = ["web:bump-client"]

[[actions]]
id = "bump-client"
key = "P"
title = "Bump the client"
ask = ["package", "version"]
task = "Bump {package} to {version}; commit."
```

## Example files

```toml
# ~/.config/kelpie/config.toml
[board.backlog]
sources = ["local", "vault"]

[worktree]
dev_url = "https://{worktree}.test"

[task]
rules_skill = "my-rules"

[review]
model = "opus"

[notify]
command = 'curl -s -d "$KELPIE_TEXT" https://ntfy.sh/my-kelpie-topic'

[update]
channel = "release"

[restart]
auto = true
idle_minutes = 10
```

```toml
# ~/.config/kelpie/repos/app.toml
[worktree]
setup_command = "script/prepare_worktree"
dev_server = "bin/dev"
dev_worker = "bin/jobs"
```

## Claude Code plugin option

| Option | Default | Meaning |
| --- | --- | --- |
| `auto_open` | `false` | Open the review split next to a task's Claude on its first file edit in a session, without taking focus. Only inside herdr and in a git worktree. Closing the pane keeps it closed for that session. |

Set it under `/plugin` → Installed → kelpie → Configure, or at install with `claude plugin install kelpie@kelpie --config auto_open=true`.
