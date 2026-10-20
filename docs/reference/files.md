# Files and paths

## Paths

| What | Default path | Override |
| --- | --- | --- |
| Machine config | `~/.config/kelpie/config.toml` | `KELPIE_CONFIG`, `XDG_CONFIG_HOME` |
| Repo configs | `~/.config/kelpie/repos/<repo>.toml` | `KELPIE_REPOS_DIR`, `XDG_CONFIG_HOME` |
| Local backlog | `backlog.md` next to the machine config | follows the machine config |
| State directory | `~/.local/state/kelpie` | `KELPIE_STATE_DIR`, `XDG_STATE_HOME` |
| The lead's working directory | `lead` in the state directory | `KELPIE_LEAD_DIR` |
| Install link | `~/.local/share/kelpie/current`, written by herdr's startup | none: kelpie reads it under `~/.local/share` |
| The binary | `~/.local/share/kelpie/current/target/release/kelpie` | `KELPIE_BIN` for roles and skills |
| Sidebar sync state | `~/.local/state/herdr-sync` | `XDG_STATE_HOME` |
| A worktree's setup note | `$(git rev-parse --git-dir)/kelpie-setup.md`, run inside the worktree | none |
| Worktrees | `wt<N>.<repo>` next to the main checkout | none |
| Vault index | `~/.cache/notes-vault-mcp` | the vault plugin's options, or `VAULT_CACHE_DIR` |

## The state directory

The folder is `0700` and its files `0600`. Each file is replaced whole on write.

| Entry | Holds |
| --- | --- |
| `messages.jsonl` | Every message between sessions. |
| `turns/` | Task reports. |
| `roles/` | The task and lead roles. |
| `reviews/` | Review split marks. |
| `verdicts/<task>@<repo>.md` | The reviewer's latest verdict for a task. |
| `cards/` | Sign-off cards. |
| `evidence/<task>@<repo>.jsonl` | The task's last 20 recorded test runs, one JSON object per line. See [`kelpie evidence`](commands.md#kelpie-evidence-name---json). |
| `plans/` | Plans put to you with `kelpie ask --plan <file>`. |
| `push-permits/` | Push permissions you gave the lead, per task and per repo. |
| `approved/` | The lead's rules approvals. |
| `ci/` | One file per CI watch, `<task>@<repo>.json`: the pushed sha, the GitHub repo, the runs as last seen, when it started and was last checked, and whether it failed. A watch survives board restarts. |
| `away` | Present while away mode is on. |
| `backlog.json` | The backlog the lead saved on its last refresh. |
| `recover/` | One file per task an API error stopped, `<task>@<repo>.json`: the error, how many times kelpie resumed the task, when it resumes next and whether it is stuck. It survives restarts, so kelpie never resumes a task twice for one error. |
| `usage.json` | Plan usage from the status line. |
| `update-check.json` | The last update check. |
| `update-override.json` | The `--from` override, while it holds. |
| `notify.log` | Notify command failures. |
| `keeper.lock` | Held by the running keeper, with its pid, binary and start time. |
| `keeper.json` | The keeper's routines, each with its last run and error, and the stale sessions it last saw. |
| `keeper.log` | What the keeper did: starts, exits, routine errors and restarts. Past 256 KB it moves to `keeper.log.1`. |
| `lead/` | The lead's working directory. |

## The local backlog file

`backlog.md` next to the machine config. kelpie reads it as it changes and only appends to it; other lines are yours.

One item per line:

```markdown
- [high] Fix login — the session drops on reload (app)
- Write the release notes (web)
```

| Part | Rule |
| --- | --- |
| `[priority]` | `urgent`, `high`, `medium` or `low`. Optional. |
| Title | Required. kelpie knows the item by its title, so keep titles unique. |
| ` — summary` | Optional. |
| `(area)` | The folder name of the repo the work happens in. Optional, but starting the item as a task needs it. |

## The verdict file

`verdicts/<task>@<repo>.md` in the state directory:

| Part | Value |
| --- | --- |
| `verdict:` | `ok` or `changes` |
| sha | The commit the reviewer read |
| Findings | Each with `file:line`, bugs first |

## The setup note

`kelpie-setup.md` in the worktree's git dir, outside `git status`. The setup skill writes what it did there; the next task in that worktree reads it and checks only what drifted. `kelpie list` shows its first line.

## Shipped with kelpie

| Path in the install root | Purpose |
| --- | --- |
| `examples/herdr-config.toml` | A complete herdr config with kelpie's keys, the sidebar sync and a sidebar layout. |
| `scripts/sync.sh` | The sidebar sync, run from herdr's tab bar. |
| `roles/lead.md`, `roles/task.md` | The lead's and a task's role templates. |
| `skills/` | The Claude Code plugin's skills. See [Skills](skills.md). |
