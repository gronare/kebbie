# Files and paths

Where kebbie keeps its config, state and the files it ships, and what each file holds.

## Paths

| What | Default path | Override |
| --- | --- | --- |
| Machine config | `~/.config/kebbie/config.toml` | `KEBBIE_CONFIG`, `XDG_CONFIG_HOME` |
| Repo configs | `~/.config/kebbie/repos/<repo>.toml` | `KEBBIE_REPOS_DIR`, `XDG_CONFIG_HOME` |
| Local backlog | `backlog.md` next to the machine config | follows the machine config |
| State directory | `~/.local/state/kebbie` | `KEBBIE_STATE_DIR`, `XDG_STATE_HOME` |
| The lead's working directory | `lead` in the state directory | `KEBBIE_LEAD_DIR` |
| Install link | `~/.local/share/kebbie/current`, written by herdr's startup | none: kebbie reads it under `~/.local/share` |
| The binary | `~/.local/share/kebbie/current/target/release/kebbie` | `KEBBIE_BIN` for roles and skills |
| Sidebar sync state | `~/.local/state/herdr-sync` | `XDG_STATE_HOME` |
| A worktree's setup note | `$(git rev-parse --git-dir)/kebbie-setup.md`, run inside the worktree | none |
| Worktrees | `wt<N>.<repo>` next to the main checkout | none |
| Vault index | `~/.cache/notes-vault-mcp` | the vault plugin's options, or `VAULT_CACHE_DIR` |

## The state directory

The folder is `0700` and its files `0600`. Each file is replaced whole on write.

### Tasks

| Entry | Holds |
| --- | --- |
| `sessions/` | One file per task, `<task>@<repo>`: its Claude session id, permission mode, profile and agent as JSON, so a restart resumes the same conversation with the same profile. |
| `phase/` | One file per task, `<task>@<repo>.json`: where it stands, since when, and what moved it there. The board's group follows it. |
| `turns/` | Task reports. |
| `roles/` | The task and lead roles. |
| `cards/` | Sign-off cards. |
| `plans/` | Plans put to you with `kebbie ask --plan <file>`. |
| `prompts/` | Per task, the id of the prompt on its Claude's screen that the lead was last told about. |
| `evidence/<task>@<repo>.jsonl` | The task's last 20 recorded test runs, one JSON object per line. See [`kebbie evidence`](commands.md#kebbie-evidence-name---json). |
| `evidence/<task>@<repo>.hook.json` | When the hook that records test runs last ran in the task, and the last 3 commands that looked like test runs but are not in `task.test_commands`. |
| `after/` | One file per queued task, `<task>@<repo>.json`. |
| `recover/` | One file per task an API error stopped, `<task>@<repo>.json`. |
| `started/` | Per task half, the vault note it was started from or opened. |
| `vault-gate/` | Per task, the mark that its note's Problem is written, so its edits go through. |
| `halves/` | Per task, every worktree it has across repos, so each half's file tools take the others' worktrees as the task's own. |
| `landed/` | Per task: why the keeper leaves a landed task open, at which head and since when, and the mark of a task pushed to keep open. |
| `follow-main/` | Per task, the flag that it follows main; `follow-main/handed/` the conflicts already handed to its Claude, once per new main. |
| `closing/`, `fetch/` | Locks: one per task while it closes, one per repo while it fetches. |

- `sessions/`: a file of two lines, the id and the mode, reads as the `task` profile on Claude. `<task>@<repo>.prompt` keeps its first prompt.
- `phase/` holds `name`, with `why`, `head` or `sha`. What moved it there is its start, a turn, a report, a question or its answer, a push, a CI verdict or an API error. A task from before this file existed gets one the first time the board or `kebbie status` reads it.
- `after/` holds its repo, the tasks it waits for by address and repo, its prompt, mode and backlog item, who queued it (you or the lead), when, and, once it waits for you, why. A file that does not read as one is removed unread. The keeper renames a file away before it starts the task, so a board and a keeper never start it twice.
- `recover/` holds the error, how many times kebbie resumed the task, when it resumes next and whether it is stuck. It survives restarts, so kebbie never resumes a task twice for one error.

### Review and rules

| Entry | Holds |
| --- | --- |
| `reviews/` | Review split marks. |
| `verdicts/<task>@<repo>.md` | The reviewer's latest verdict for a task. See [The verdict file](#the-verdict-file). |
| `approved/` | The rules approvals: the reviewer's pass or `kebbie approve`. |
| `rules/` | The reviewer's rules checks, per `<task>@<repo>`, by extension below. |

| `rules/` file | Holds |
| --- | --- |
| `.wanted` | A ready report waiting for the keeper. |
| `.again` | One that came in during a check. |
| `.run` | The keeper's record of the run it started. |
| `.running` | The check under way. |
| `.md` | The last verdict. |
| `.failed` | The code it gave no verdict on. |
| `.lead` | The code the lead was asked about. |
| `.log` | Its output. |

### Pushes, CI and staging

| Entry | Holds |
| --- | --- |
| `push-permits/` | Push permissions you gave the lead, per task and per repo. |
| `push-intents/` | One file per push that waits for its task's Claude to test the files the default branch changed under it: who asked (you or the lead), the commits it holds for, and which commits to push. |
| `push-line/` | Per repo, the lock of its push line. |
| `push-left/` | Per task, why a push left its line unpushed, for its Needs you row. |
| `push-tokens/` | Per push, the token the worktree's push block lets through only while kebbie runs that push. |
| `ci/` | One file per CI watch, `<task>@<repo>.json`. A watch survives board restarts. |
| `red-ci/` | Per task, `<task>@<repo>.json`: the reason its Claude gave with `kebbie ci outside` and the red runs you dismissed. |
| `staged/` | Per repo, `<repo>.json`: the tasks [`kebbie stage`](commands.md#kebbie-stage-name) put on local main, in the order they went on. |

A `ci/` file holds the pushed sha, the GitHub repo, the runs as last seen, when it started and was last checked, and whether it failed.

### Messages and sessions

| Entry | Holds |
| --- | --- |
| `messages.jsonl` | Every message between sessions. |
| `messages.jsonl.1`, `messages.lock` | The previous message log, kept when the log rotates, and the lock its writers take. |
| `typed/` | One file per message kebbie typed into a Claude's input box or gave to its inbox, while the board checks the conversation for it. |
| `inbox/` | Per session, `<address>.json`: the Claude Code inbox socket, its token and the session id its last start recorded, which kebbie delivers messages through. |
| `lead-session` | The lead's Claude session id, so it resumes its conversation. |
| `lead-previous` | The Space you were in before `kebbie lead`, to go back to. |
| `lead/` | The lead's working directory, where kebbie starts its Claude every time. |
| `lead.log` | What herdr's startup did to bring the board back in the lead Space: a line when it started the board, and any error. |
| `restart/` | The restarts, below. |
| `away` | Present while away mode is on. |

`restart/` holds `updated`, when `kebbie update` last ran; `asked.json`, the sessions the board asked you about; `restarted.json`, the restarts done; and one lock per session while it restarts.

### The keeper, board and backlog

| Entry | Holds |
| --- | --- |
| `keeper.lock` | Held by the running keeper, with its pid, binary and start time. |
| `keeper.json` | The keeper's routines, each with its last run and error, and the stale sessions it last saw. |
| `keeper.log` | What the keeper did: starts, exits, routine errors and restarts. Past 256 KB it moves to `keeper.log.1`. |
| `routines/` | One file per routine, `<id>.json`. `lock` keeps two schedulers from starting the same run. |
| `backlog.json` | The backlog the lead saved on its last refresh. |
| `board-messages` | The board's memory of whether the messages panel is folded. |
| `board-reports` | Whether the `B` list shows. |
| `cell-size` | The terminal cell size a board in a normal pane last measured, for the popup board's images. |
| `usage.json` | Plan usage from the status line. |
| `update-check.json` | The last update check. |
| `update-override.json` | The `--from` override, while it holds. |
| `notify.log` | Notify command failures. |
| `notify-sent.json` | When each kind of notification last went out, for the limit of one per event a minute. |
| `vault-status.json` | Whether the vault plugin is on and writes claims, kept a while so a start or a push does not ask the vault each time. |
| `vault-logs/` | Each repo's vault log note. |
| `vault.log` | The vault calls that failed, one line each. |

A `routines/` file holds when the scheduler first saw the routine, when its last run started, that run's task, its outcome (`running`, `ok`, `findings` or `failed`) and summary, and the four runs before it, which the board's Routines page lists.

### Issues

`issues/` holds one file per [kebbie issue](commands.md#kebbie-issues), `<id>.md`. See [The issue file](#the-issue-file). The result of a recheck lives in the issue file's `rechecked` key.

| Entry | Holds |
| --- | --- |
| `issues/held` | The one line the lead holds to propose at a calm moment (`kebbie issue hold`), removed when it is taken or an issue is saved or reopened. |
| `issues/recheck/` | The rechecks under way, per issue, by extension below, and `dismissed`, when you last cleared the board's line. |

| `issues/recheck/` file | Holds |
| --- | --- |
| `.wanted` | The build it waits to be checked on. |
| `.run` | The keeper's record of the checker it started. |
| `.running` | The check under way. |
| `.claude` | Its Claude's process group. |
| `.log` | Its output, removed once the check gives a verdict. |
| `.note` | A failure the keeper logs once. |

### Reports to kebbie's maintainers

`report/` holds reports, error signals and the install signal to kebbie's maintainers. `kebbie hub leave` leaves it.

| Entry | Holds |
| --- | --- |
| `install-key.json` | This install's signing key, made on the first report or signal, mode 0600. |
| `signals.jsonl` | The failures counted since the last send, one line each with version, commit, component, kind and count. |
| `signals-batch.jsonl` | The counts a send is under way for. |
| `signals.json` | The time of the last send and its error. |
| `install-id` | The install signal's random id, made on its first send. |
| `install-signal.json` | The day the install signal last went. |
| `always-armed` | The date you ran `kebbie issue report --always`, while it holds. |
| `inbox.json` | With `[report] inbox_token` set: the reports filed here by their report id, the issues filed from error signals, how many reports wait in quarantine on the issues endpoint, and the last fetch and its error. |

### The hub

`hub/` holds the kebbie hub this machine joined. `kebbie hub leave` removes the folder. See [See your board on your phone](../how-to/see-your-board-on-your-phone.md).

| Entry | Holds |
| --- | --- |
| `hub/identity.json` | The hub's address, the machine's id and name there, its token, and its signing and encryption keys. |
| `hub/relay.json` | The board's last revision, when it went out, its fingerprint, the last error and whether the hub refused it for a lapsed subscription. |
| `hub-dev/` | The same for the dev hub, used instead of `hub/` while [`KEBBIE_HUB_DEV`](environment.md) is set. |

When the machine was connected from its link, `identity.json` also holds the account key it pinned, with the account's id, the key's version, its public keys, their two fingerprints and when it was pinned.

## The local backlog file

`backlog.md` next to the machine config. kebbie reads it as it changes and only appends to it; other lines are yours.

One item per line:

```markdown
- [high] Fix login — the session drops on reload (app)
- Write the release notes (web)
```

| Part | Rule |
| --- | --- |
| `[priority]` | `urgent`, `high`, `medium` or `low`. Optional. |
| Title | Required. kebbie knows the item by its title, so keep titles unique. |
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
github = "https://github.com/gronare/kebbie/issues/12"
sent = "2026-10-21"
history = ["fixed in 0.26.3 (a1b2c3d), checked automatically"]
rechecked = "a1b2c3d fixed 1792540800 0.26.3"
+++

What happened: …
```

| Key | Value |
| --- | --- |
| `id` | Six hex characters, kept for the issue's life. |
| `kind` | `bug` or `wish`. |
| `status` | `open`, `retry`, `fixed`, `wontfix` or `duplicate`. |
| `version`, `commit` | The kebbie it was filed on. `commit` is empty for a build outside git. |
| `by` | `user`, `lead`, or `report` for one filed from a report. |
| `github` | The linked GitHub issue. Optional. |
| `sent` | The date of the last `kebbie issue send` or `kebbie issue report`. Optional. |
| `report` | The report id it went to kebbie's maintainers under. Set once; the issue is never sent again. Optional. |
| `declined` | The date you answered `n` to sending it. The board offers it no more. Optional. |
| `source` | `report <id>` for an issue the keeper filed from a report another install sent, under the id `r<id>`. Optional. |
| `rechecked` | The last recheck after an update: the build's commit, the verdict, when, and the build's version. Optional. |
| `history` | What happened to it since it was filed, oldest first. Optional. |

- `id`: the `kebbie-issue: <id>` line in a sent issue is how `kebbie update` finds it on GitHub.
- `source`: such an issue shows as `[report <id>, untrusted]`, its text fenced. It is never offered for sending, and never rechecked after an update.
- `rechecked`: the verdict is `started`, `fixed`, `not-fixed`, `unclear` or `failed`, and the time is in seconds since 1970. An issue is rechecked once per build. The board's `Fixed in kebbie <version>: N bugs you ran into` counts this machine's own issues still `fixed` by a recheck in the last three days and since you last cleared it, under the latest fix's version.
- `history` holds `fixed in <version> (<sha>), checked automatically` from a recheck, and `came back on <version> (<commit>)` from `kebbie issue reopen`, with `(was report <id>)` when it had been sent. A reopen clears `report`, `sent` and `declined`, so the bug is offered for sending again.

## The verdict file

`verdicts/<task>@<repo>.md` in the state directory:

| Part | Value |
| --- | --- |
| `verdict:` | `ok` or `changes` |
| sha | The commit the reviewer read |
| Findings | Each with `file:line`, bugs first |

## The setup note

`kebbie-setup.md` in the worktree's git dir, outside `git status`. The setup skill writes what it did there; the next task in that worktree reads it and checks only what drifted. `kebbie list` shows its first line.

## Shipped with kebbie

| Path in the install root | Purpose |
| --- | --- |
| `examples/herdr-config.toml` | A complete herdr config with kebbie's keys, the sidebar sync and a sidebar layout. |
| `examples/profiles/` | Two [profiles](configuration.md#profiles) to copy into `~/.config/kebbie/profiles/`: `designer` for UI work and `careful` for risky changes, each with its role file. |
| `scripts/sync.sh` | The sidebar sync, run from herdr's tab bar. |
| `scripts/build.sh` | herdr's build step for the plugin: puts the binary at `target/release/kebbie`, the release's prebuilt one on a release tag, else `cargo build --release`. |
| `scripts/herdr-startup.sh` | herdr's startup step: links `~/.local/share/kebbie/current` to the install and brings the board back in the lead Space. |
| `scripts/review.sh` | Opens the review split next to Claude; the view itself is `kebbie review --view`. |
| `scripts/review.lua` | The review split's diff pane in nvim. |
| `scripts/check.sh` | What CI checks, run locally before a push: tests, clippy, `cargo audit`, `cargo deny` and a build on the oldest Rust the crate supports. See [Develop kebbie](../how-to/develop-kebbie.md). |
| `roles/lead.md`, `roles/task.md` | The lead's and a task's role templates. |
| `skills/` | The Claude Code plugin's skills. See [Skills](skills.md). |
