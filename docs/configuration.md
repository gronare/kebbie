# Configuration

Everything works without a config file. When you want to change something, kelpie reads its settings from this machine only, never from a file inside a repo, so a repo carries nothing that applies to only some of the people who clone it.

## Layers

Read key by key; the first layer that sets a key wins:

1. **The repo config**, `~/.config/kelpie/repos/<repo>.toml`, where `<repo>` is the main checkout's folder name. Every worktree of `~/code/app` reads `repos/app.toml`. It holds `[worktree]`, `[task]`, `[review]` and the repo's [`[[actions]]`](areas.md#your-own-actions).
2. **The machine config**, `~/.config/kelpie/config.toml` (or `$XDG_CONFIG_HOME/kelpie/config.toml`).
3. kelpie's built-in defaults.

Keys that apply to every repo (`[board]`, `[notify]`, `[update]`, `[restart]`, `[vault]`, `server_name`, `worker_name`) belong in config.toml; in a repo config they are an error that says so. An unknown key in either file is an error that names the file and the key.

## See what is in effect

```bash
kelpie config                  # every key, its value, the layer it comes from, what it does
kelpie config --repo ~/code/app
kelpie config --get worktree.server_name   # server_name or worker_name, for scripts
```

`kelpie doctor` checks config.toml and every file in the repos folder, a broken link included.

## Keys

| Key | Where | Default | What it does |
| --- | --- | --- | --- |
| `board.backlog.sources` | config.toml | `["local"]` | Where the board's Backlog finds work: `"local"`, `"vault"`, `"linear"`, in any combination. The first is where the lead files new ideas; `[]` turns it off. See [Backlog](backlog.md). |
| `worktree.setup_skill` | either | `"kelpie:set-up-worktree"` | The skill a task's Claude runs first to make its worktree runnable. `plugin:skill` for a plugin's skill, a bare name for one in `~/.claude/skills` or the repo's `.claude/skills`. |
| `worktree.setup_command` | either | none | The repo's own command that prepares a worktree, run from inside it by the setup skill. A path applies only in a repo that has the file; elsewhere, and when none, `bin/setup-worktree` when the repo has one. |
| `worktree.dev_server` | either | none | The command the board's `w` runs in a `server` tab of the task's Space, with `PORT` set to the worktree's port. Without one there is no `w`. |
| `worktree.dev_worker` | either | none | A second command, such as a job runner, that `b` runs in a `worker` tab, with the same `PORT`. Without one there is no `b`. |
| `worktree.dev_url` | either | `"http://localhost:{port}"` | The running dev server's address, shown on the board as a link. `{worktree}`, `{repo}`, `{number}` and `{port}` are filled in; `""` turns the link off. See [Worktrees](worktrees.md#urls). |
| `worktree.server_name` | config.toml | `"server"` | The name of the tab `w` starts the dev server in, and of the Space's tag in herdr's sidebar while it runs. |
| `worktree.worker_name` | config.toml | `"worker"` | The same for `b`'s worker tab. |
| `task.rules_skill` | either | `"kelpie:work-rules"` | The skill with your rules for how work is planned, built and committed. A task's Claude follows it, the lead checks a finished task against it, `tidy` reshapes commits by it, and the reviewer reads it. |
| `review.model` | either | none | The model the [reviewer](review.md#the-reviewer-gate) runs on. None leaves it to Claude's default. |
| `notify.command` | config.toml | none | The shell command kelpie runs when something needs you. See [Notifications](lead.md#notifications). |
| `vault.command` | config.toml | none | The notes-vault-mcp command for task notes and log lines. None runs the vault plugin's pinned command. See [Vault](vault.md). |
| `update.channel` | config.toml | `"release"` | What `kelpie update` installs: `"release"`, the highest `vX.Y.Z` tag on origin, or `"main"`, origin's default branch. See [Updating](updating.md). |
| `restart.auto` | config.toml | `true` | Whether the board restarts stale Claude sessions on its own and `kelpie update` restarts idle ones at once. `false` leaves it to `kelpie restart`. |
| `restart.idle_minutes` | config.toml | `10` | How long a stale session must be idle and out of focus before the board restarts it, 1 to 1440. |

## Examples

A machine config:

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
```

A repo config:

```toml
# ~/.config/kelpie/repos/app.toml
[worktree]
setup_command = "script/prepare_worktree"
dev_server = "bin/dev"
dev_worker = "bin/jobs"
```

## Team setup

A team shares its kelpie setup through a team repo. The repo configs live there, with one short file that lists which file links to which kelpie path. Each developer clones the repo and links the files in:

```bash
ln -s ~/code/team-config/kelpie/repos/app.toml ~/.config/kelpie/repos/app.toml
```

A pull in the team repo updates every linked setting. Either config file may be a symlink; `kelpie config` and `kelpie doctor` show each link with its target.

## Let Claude do it

The [`kelpie:configure`](../skills/configure/SKILL.md) skill changes a setting or sets a machine up, and verifies each change with `kelpie doctor`. Ask Claude to "set the dev server for app to bin/dev", or to "adopt the team's kelpie setup", and it finds the team repo's file, makes the links and checks them.

## Environment

| Variable | Default | What it does |
| --- | --- | --- |
| `KELPIE_CONFIG` | `$XDG_CONFIG_HOME/kelpie/config.toml`, else `~/.config/kelpie/config.toml` | The machine config. The local `backlog.md` is read next to it. |
| `KELPIE_REPOS_DIR` | `$XDG_CONFIG_HOME/kelpie/repos`, else `~/.config/kelpie/repos` | The folder of repo configs. |
| `KELPIE_STATE_DIR` | `$XDG_STATE_HOME/kelpie`, else `~/.local/state/kelpie` | kelpie's state: messages, reports, roles, verdicts, review marks, `notify.log`. The folder is `0700` and its files `0600`. Point it at a temp dir for a throwaway run. |
| `KELPIE_REPO_DIRS` | `~/projects:~/personal:~/src:~/code` | Colon-separated folders where `start --repo` looks for checkouts, in them or one level down. |
| `KELPIE_LEAD_DIR` | `lead` in the state directory | The lead Claude's working directory. |
| `KELPIE_BIN` | `~/.local/share/kelpie/current/target/release/kelpie` | The binary the lead's role and kelpie's skills run. |
| `KELPIE_CLAUDE_BIN` | `claude` | The Claude Code binary `kelpie setup vault` runs. |
| `KELPIE_VAULT_BIN` | none | A vault command that stands in for the plugin, for tests. |
| `CLAUDE_CONFIG_DIR` | `~/.claude` | Where kelpie reads Claude Code's installed plugins and their settings. |
| `XDG_DATA_HOME` | `~/.local/share` | Where herdr's startup writes the `current` link. kelpie reads it under `~/.local/share`, so `kelpie doctor` warns when this is set elsewhere. |

`KELPIE_BOARD_STATE` and `KELPIE_PUSH_TOKEN` are handoffs between kelpie's own processes; never set them.
