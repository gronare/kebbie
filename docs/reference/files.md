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
| `sessions/` | One file per task, `<task>@<repo>`: its Claude session id, permission mode, profile and agent as JSON, so a restart resumes the same conversation with the same profile. A file of two lines, the id and the mode, reads as the `task` profile on Claude. `<task>@<repo>.prompt` keeps its first prompt. |
| `reviews/` | Review split marks. |
| `verdicts/<task>@<repo>.md` | The reviewer's latest verdict for a task. |
| `cards/` | Sign-off cards. |
| `evidence/<task>@<repo>.jsonl` | The task's last 20 recorded test runs, one JSON object per line. See [`kelpie evidence`](commands.md#kelpie-evidence-name---json). |
| `plans/` | Plans put to you with `kelpie ask --plan <file>`. |
| `push-permits/` | Push permissions you gave the lead, per task and per repo. |
| `approved/` | The rules approvals: the reviewer's pass or `kelpie approve`. |
| `rules/` | The reviewer's rules checks, per `<task>@<repo>`: `.wanted` a ready report waiting for the keeper, `.again` one that came in during a check, `.run` the keeper's record of the run it started, `.running` the check under way, `.md` the last verdict, `.failed` the code it gave no verdict on, `.lead` the code the lead was asked about, `.log` its output. |
| `prompts/` | Per task, the id of the prompt on its Claude's screen that the lead was last told about. |
| `push-intents/` | One file per push that waits for its task's Claude to test the files the default branch changed under it: who asked (you or the lead), the commits it holds for, and which commits to push. |
| `after/` | One file per queued task, `<task>@<repo>.json`: its repo, the tasks it waits for by address and repo, its prompt, mode and backlog item, who queued it (you or the lead), when, and, once it waits for you, why. A file that does not read as one is removed unread. The keeper renames a file away before it starts the task, so a board and a keeper never start it twice. |
| `phase/` | One file per task, `<task>@<repo>.json`: where it stands (`name`, with `why`, `head` or `sha`), since when, and what moved it there: its start, a turn, a report, a question or its answer, a push, a CI verdict, an API error. The board's group follows it. A task from before this file existed gets one the first time the board or `kelpie status` reads it. |
| `ci/` | One file per CI watch, `<task>@<repo>.json`: the pushed sha, the GitHub repo, the runs as last seen, when it started and was last checked, and whether it failed. A watch survives board restarts. |
| `away` | Present while away mode is on. |
| `backlog.json` | The backlog the lead saved on its last refresh. |
| `recover/` | One file per task an API error stopped, `<task>@<repo>.json`: the error, how many times kelpie resumed the task, when it resumes next and whether it is stuck. It survives restarts, so kelpie never resumes a task twice for one error. |
| `routines/` | One file per routine, `<id>.json`: when the scheduler first saw it, when its last run started, that run's task, its outcome (`running`, `ok`, `findings` or `failed`) and summary, and the four runs before it, which the board's Routines page lists. `lock` keeps two schedulers from starting the same run. |
| `issues/` | One file per [kelpie issue](commands.md#kelpie-issues), `<id>.md`. See [The issue file](#the-issue-file). `issues/recheck/` holds the rechecks under way, per issue: `.wanted` the build it waits to be checked on, `.run` the keeper's record of the checker it started, `.running` the check under way, `.claude` its Claude's process group, `.log` its output, `.note` a failure the keeper logs once; and `dismissed`, when you last cleared the board's line. The result of a recheck lives in the issue file's `rechecked` key. |
| `usage.json` | Plan usage from the status line. |
| `update-check.json` | The last update check. |
| `update-override.json` | The `--from` override, while it holds. |
| `notify.log` | Notify command failures. |
| `report/` | Reports and error signals to kelpie's maintainers: `install-key.json` holds this install's signing key, made on the first report or signal, mode 0600; `signals.jsonl` the failures counted since the last send, one line each with version, commit, component, kind and count, `signals-batch.jsonl` the counts a send is under way for, `signals.json` the time of the last send and its error, and `always-armed` the date you ran `kelpie issue report --always`, while it holds; with `[report] inbox_token` set, `inbox.json` holds the reports filed here by their report id, how many wait in quarantine on the hub, and the last fetch and its error. `kelpie hub leave` leaves it. |
| `hub/` | The kelpie hub this machine joined: `identity.json` holds the hub's address, the machine's id and name there, its token, its signing and encryption keys, the secret its notices' collapse tags come from, and each paired phone's id and public keys; `relay.json` the board's last revision, when it went out, its fingerprint, the last error and the notices sent. `kelpie hub leave` removes the folder. See [See your board on your phone](../how-to/see-your-board-on-your-phone.md). |
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
history = ["fixed in 0.26.3 (a1b2c3d), checked automatically"]
rechecked = "a1b2c3d fixed 1792540800 0.26.3"
+++

What happened: …
```

| Key | Value |
| --- | --- |
| `id` | Six hex characters, kept for the issue's life. The `kelpie-issue: <id>` line in a sent issue is how `kelpie update` finds it on GitHub. |
| `kind` | `bug` or `wish`. |
| `status` | `open`, `retry`, `fixed`, `wontfix` or `duplicate`. |
| `version`, `commit` | The kelpie it was filed on. `commit` is empty for a build outside git. |
| `by` | `user`, `lead`, or `report` for one filed from a report. |
| `github` | The linked GitHub issue. Optional. |
| `sent` | The date of the last `kelpie issue send` or `kelpie issue report`. Optional. |
| `report` | The report id it went to kelpie's maintainers under. Set once; the issue is never sent again. Optional. |
| `declined` | The date you answered `n` to sending it. The board offers it no more. Optional. |
| `source` | `report <id>` for an issue the keeper filed from a report another install sent, under the id `r<id>`. Such an issue shows as `[report <id>, untrusted]`, its text fenced; it is never offered for sending, and never rechecked after an update. Optional. |
| `rechecked` | The last recheck after an update: the build's commit, the verdict (`started`, `fixed`, `not-fixed`, `unclear` or `failed`), when, as seconds since 1970, and the build's version. An issue is rechecked once per build. The board's `kelpie <version> fixed N of your issues` counts your issues still `fixed` by a recheck in the last three days and since you last cleared it, under the latest fix's version. Optional. |
| `history` | What happened to it since it was filed, oldest first: `fixed in <version> (<sha>), checked automatically` from a recheck, `came back on <version> (<commit>)` from `kelpie issue reopen`. Optional. |

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
| `examples/profiles/` | Two [profiles](configuration.md#profiles) to copy into `~/.config/kelpie/profiles/`: `designer` for UI work and `careful` for risky changes, each with its role file. |
| `scripts/sync.sh` | The sidebar sync, run from herdr's tab bar. |
| `roles/lead.md`, `roles/task.md` | The lead's and a task's role templates. |
| `skills/` | The Claude Code plugin's skills. See [Skills](skills.md). |
