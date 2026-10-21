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
| `prompts/` | Per task, the id of the prompt on its Claude's screen that the lead was last told about. |
| `push-intents/` | One file per push that waits for its task's Claude to test the files the default branch changed under it: who asked (you or the lead), the commits it holds for, and which commits to push. |
| `ci/` | One file per CI watch, `<task>@<repo>.json`: the pushed sha, the GitHub repo, the runs as last seen, when it started and was last checked, and whether it failed. A watch survives board restarts. |
| `away` | Present while away mode is on. |
| `backlog.json` | The backlog the lead saved on its last refresh. |
| `recover/` | One file per task an API error stopped, `<task>@<repo>.json`: the error, how many times kelpie resumed the task, when it resumes next and whether it is stuck. It survives restarts, so kelpie never resumes a task twice for one error. |
| `routines/` | One file per routine, `<id>.json`: when the scheduler first saw it, when its last run started, that run's task, its outcome (`running`, `ok`, `findings` or `failed`) and summary, and the four runs before it, which the board's Routines page lists. `lock` keeps two schedulers from starting the same run. |
| `issues/` | One file per [kelpie issue](commands.md#kelpie-issues), `<id>.md`. See [The issue file](#the-issue-file). |
| `usage.json` | Plan usage from the status line. |
| `update-check.json` | The last update check. |
| `update-override.json` | The `--from` override, while it holds. |
| `notify.log` | Notify command failures. |
| `keeper.lock` | Held by the running keeper, with its pid, binary and start time. |
| `keeper.json` | The keeper's routines, each with its last run and error, and the stale sessions it last saw. |
| `keeper.log` | What the keeper did: starts, exits, routine errors and restarts. Past 256 KB it moves to `keeper.log.1`. |
| `lead.log` | What herdr's startup did to bring the board back in the lead Space: a line when it started the board, and any error. |
| `lead/` | The lead's working directory, where kelpie starts its Claude every time. |

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

## The issue file

`issues/<id>.md` in the state directory: TOML between `+++` lines, then the text.

```markdown
+++
id = "3f9a2c"
title = "push hides git's error when the rebase fails"
kind = "bug"
status = "open"
version = "0.24.0"
commit = "869bc49"
filed = "2026-10-21"
by = "lead"
github = "https://github.com/gronare/kelpie/issues/12"
sent = "2026-10-21"
+++

What happened: …
```

| Key | Value |
| --- | --- |
| `id` | Six hex characters, kept for the issue's life. The `kelpie-issue: <id>` line in a sent issue is how `kelpie update` finds it on GitHub. |
| `kind` | `bug` or `wish`. |
| `status` | `open`, `retry`, `fixed`, `wontfix` or `duplicate`. |
| `version`, `commit` | The kelpie it was filed on. `commit` is empty for a build outside git. |
| `by` | `user` or `lead`. |
| `github` | The linked GitHub issue. Optional. |
| `sent` | The date of the last `kelpie issue send`. Optional. |

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
