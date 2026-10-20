# kelpie

Herding coding agents in [herdr](https://herdr.dev). A kelpie is the Australian working dog that keeps a herd moving while the drover directs; here herdr holds the agents and kelpie keeps their work moving towards you.

kelpie runs each task in its own git worktree, herdr Space and Claude. A lead Claude holds the whole picture and talks to you; a board shows every task by what it needs from you, next to a backlog; a separate read-only Claude reviews a task before it is pushed; and a review split next to a Claude pane shows its changes as diffs and sends your line comments back to it. Three parts make it up: the `kelpie` command (Rust, one binary), a herdr plugin that builds and runs it, and a Claude Code plugin with the skills and the review hook.

## Install

For macOS and Linux, in this order. Every step is checked by `kelpie doctor` at the end, which prints the fix for anything missing.

**1. The tools.** kelpie runs inside herdr and needs these on `PATH`:

| Tool | Needed | For | macOS | Linux |
| --- | --- | --- | --- | --- |
| `git` | yes | everything | `xcode-select --install` (or `brew install git`) | `sudo apt install git` |
| `herdr` 0.9.0 or later | yes | everything | `brew install herdr` | `curl -fsSL https://herdr.dev/install.sh \| sh` |
| `claude` (Claude Code) | yes | everything | `curl -fsSL https://claude.ai/install.sh \| bash` | the same |
| `jq` | yes | the sidebar sync and the review hook | `brew install jq` | `sudo apt install jq` |
| `cargo` | to install | herdr building the plugin | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh` | the same |
| `fzf` 0.71.0 or later | for review | the review split's file list | `brew install fzf` | `git clone --depth 1 https://github.com/junegunn/fzf.git ~/.fzf && ~/.fzf/install` (distro packages are older) |
| `nvim` 0.10 or later | for review | the review split's diffs | `brew install neovim` | `brew install neovim`, or a [release](https://github.com/neovim/neovim/releases) |
| `tig` | for history | the history split | `brew install tig` | `sudo apt install tig` |
| `curl` | for review | the review list reloading when files change | `brew install curl` | `sudo apt install curl` |

The scripts use POSIX `sh` and short flags, so they run with the BSD tools on macOS as well.

**2. The herdr plugin.** herdr clones the repo and builds `kelpie` with `cargo build --release`:

```bash
herdr plugin install gronare/kelpie
```

To work on kelpie itself, register a checkout instead: `herdr plugin link <path to the checkout>`, then `cargo build --release` in it.

**3. Restart herdr.** On startup the plugin links `~/.local/share/kelpie/current` to wherever herdr installed it, so configs and hooks have a stable path across updates. Keep `XDG_DATA_HOME` unset for herdr, or link it yourself: herdr's startup writes under `$XDG_DATA_HOME`, and kelpie reads `~/.local/share`.

**4. `kelpie` on your PATH.** The lead, tasks and hooks run `kelpie` by name. With `~/.local/bin` on `PATH`:

```bash
mkdir -p ~/.local/bin
ln -s ~/.local/share/kelpie/current/target/release/kelpie ~/.local/bin/kelpie
```

**5. The Claude Code plugin.** It brings the skills (`kelpie:set-up-worktree`, `kelpie:work-rules`, `kelpie:review`, `/kelpie:review-split` and the rest) and the review hook. Add it from the install root, so it is always the same code as the `kelpie` binary (Claude Code loads a folder marketplace's plugin in place at every session start):

```bash
claude plugin marketplace add ~/.local/share/kelpie/current
claude plugin install kelpie@kelpie
```

`claude plugin marketplace add https://github.com/gronare/kelpie` works too, but it follows GitHub's default branch, so it fits only a machine on the `main` channel.

**6. The herdr keys.** Add to your herdr `config.toml`:

```toml
[[keys.command]]
key = "prefix+space"
type = "plugin_action"
command = "kelpie.lead"
description = "kelpie: go to the lead, or back"

[[keys.command]]
key = "prefix+t"
type = "plugin_action"
command = "kelpie.board"
description = "kelpie board"

[[keys.command]]
key = "prefix+u"
type = "plugin_action"
command = "kelpie.update"
description = "kelpie: update and reload"
```

[`examples/herdr-config.toml`](examples/herdr-config.toml) is a whole herdr config with these keys, the [sidebar sync](#what-starts-by-itself) and a sidebar layout that shows each task's state and your next step.

**7. A machine config, if you want one.** Without it every key has its default: the backlog is the local file, the setup and rules skills are kelpie's own, and nothing notifies you. To change that, write `~/.config/kelpie/config.toml` (see [Configuration](#configuration)), for instance:

```toml
[board.backlog]
sources = ["local", "vault"]

[notify]
command = 'curl -s -H "Title: kelpie $KELPIE_EVENT" -d "$KELPIE_TEXT" https://ntfy.sh/my-kelpie-topic'
```

**8. Check.** Run `kelpie doctor` and follow its fixes until every line is `ok`. It checks the tools and their versions, the herdr and Claude Code plugins, that the plugin binary is built, the `current` link, that `kelpie` on PATH is that link's binary, the state directories, both config files, that the plugin of the setup skill in effect is installed and enabled in Claude Code, the notify command (never run), the backlog sources and, inside a repo, whether its task worktrees get the push block, and it suggests the line that adds the vault or Linear as a source when Claude Code has the vault plugin or a Linear MCP server. Each warning or failure comes with its fix for macOS and Linux.

### Update

Each machine follows a channel, set in its machine config:

```toml
[update]
channel = "release"   # the default: the highest vX.Y.Z tag on origin
# channel = "main"    # origin's default branch, every push
```

A release is a `vX.Y.Z` tag (see [Releasing](#releasing)); a push to main reaches only `main` machines. Tags that are not exactly `v` and three numbers (`v1.0`, `v1.2.3-rc1`, `nightly`) are not releases.

`kelpie update` (`prefix+u`, as a popup) brings everything to the channel's latest in one go. On `release` it fetches the tags in the install root (the folder herdr runs kelpie from), checks out the highest one detached and builds it. It refuses, and changes nothing, when the install root has uncommitted changes or is not on a release tag: on `release` the install root must not be a checkout you develop in. `kelpie update --stable` moves a clean checkout that is on a branch to the tag the first time (the branch keeps its commits). On `main` it does what it always did: a GitHub install through `herdr plugin update`, a linked checkout fast-forwarded when it is clean, then built.

To try code that is not released, `kelpie update --from <branch>` installs that branch from origin and `kelpie update --from <path>` installs the commits of another checkout (its uncommitted changes are refused, as only commits are installed; the path cannot be the install root itself). The override is kept in `update-override.json` in the state directory; while it holds, a plain `kelpie update` fetches the same branch or checkout again, and `kelpie update --check`, `kelpie doctor` and the board say `running <branch or path> @ <sha>, not the channel`. `kelpie update --stable` drops the override and returns to the channel. A branch or path starting with `-` is refused.

The Claude Code plugin is the install root's own checkout when its marketplace was added from `~/.local/share/kelpie/current`: whatever `kelpie update` checked out is what the next session and `/reload-plugins` load, a tag, main or an override alike. A GitHub marketplace follows the default branch, so `kelpie update` runs `claude plugin update` for it only on the `main` channel without an override, and otherwise leaves it as it is and prints the commands that move it to the install root (`kelpie doctor` warns about it too). Claude Code cannot follow the newest tag by itself: a marketplace added as `…kelpie.git#v0.21.0` stays on that one tag, and moving it to the next means removing the marketplace, which uninstalls the plugin and forgets its options.

Then kelpie refreshes the gronare marketplace and updates the vault plugin when it is installed, lists each version before → after (kelpie, the vault plugin and the notes-vault-mcp pin its hooks name), and sends `/reload-plugins` to every Claude session that is waiting. The lead reads its role and settings only when its Claude starts, so a waiting lead is restarted instead (`kelpie lead --restart`, run by the binary just built), resuming its conversation. The session you are in and the ones still working are listed instead, so nothing types into a prompt you are writing; for the lead that line says to run `kelpie lead --restart` when it waits.

`kelpie update --check` changes nothing but fetched refs: it names the channel, fetches kelpie's install repo and the vault plugin's marketplace clone, and shows the installed version against the channel's latest (on `release` the highest tag, on `main` origin's default branch) and the vault plugin against its marketplace's default branch. On a `release` machine the notice names the new tag (`↑ kelpie v0.21.0`), never what main carries. The board runs the same check in the background when it starts and every 6 hours after (kept in `update-check.json` in the state directory, so several boards share one fetch); when something is newer the Work status line shows a muted `↑ kelpie 0.18.2 · U updates`, and `U` runs `kelpie update` without leaving the board. `kelpie doctor` reports the last check's result without fetching.

### Releasing

A release happens only when you tag it; CI runs the tests on every push and releases nothing.

1. Bump the version in `Cargo.toml`, `herdr-plugin.toml` and `.claude-plugin/plugin.json` (a test fails when the three differ), then `cargo build` so `Cargo.lock` follows.
2. Commit and push to main: `git commit -am "kelpie 0.21.0" && git push`. Machines on `main` can try it from here.
3. Tag that commit and push the tag: `git tag v0.21.0 && git push origin v0.21.0`. Machines on `release` show `↑ kelpie v0.21.0` within 6 hours, and `kelpie update` installs it.

## First task

Press `prefix+space`. The first time, kelpie creates the lead Space: the board on the left, the lead's Claude on the right; `prefix+tab` moves focus between them. Tell the lead what you want done; it writes the task prompt and starts the task. Or start one yourself from a repo's main checkout:

```bash
kelpie start fix-login "The session drops on reload; find out why and fix it."
```

kelpie puts a worktree (`wt1.<repo>` next to the checkout) on a new branch `fix-login`, opens it as a Space, and starts Claude there. Its first step makes the worktree runnable ([Worktree setup](#worktree-setup)); at the end of every turn it reports where it stands, which the board (`prefix+t`) shows under the task. When it reports ready, the board's Push shows its sign-off card and commits, and `y` pushes them and closes the task, freeing its worktree for the next one.

## Tasks in worktrees

| Command | What it does |
| --- | --- |
| `kelpie start <name> [prompt]` | Takes a free worktree from the pool (`wt<N>.<repo>` next to the main checkout), or creates a bare one with `git worktree add --detach`. Puts it on a new branch `<name>` from the remote's default branch, opens it as a Space labelled `<name>`, and starts Claude with the prompt and [`roles/task.md`](roles/task.md): which task and repo it is, its worktree and the setup skill to run before anything else (see [Worktree setup](#worktree-setup)), its peers, and how to message them. `--repo <name>` starts in that repo instead of the current one (repeat it for one task across repos, e.g. `--repo api --repo web`: a half in each, the same branch name, each Claude told of the other): a checkout named so under `~/projects`, `~/personal`, `~/src` or `~/code` (or one level down; `KELPIE_REPO_DIRS` overrides the list), else a clone from the first owner among your other checkouts' remotes that has it. `--from <ref>` branches off that ref instead, `--backlog <item>` ties the task to a backlog item (as `kelpie link` does), `--mode <mode>` is Claude's permission mode (the task's Claude does not push: kelpie blocks `git push` in its worktree with a pre-push hook and a push URL rewrite in the worktree's own config, which kelpie's own push gets past. The block stops an accidental push, not a determined Claude running as you, which can rewrite the URL back, set a push URL or forge the hook's token. To push a task's branch for a pull request, run `git -C <main checkout> push origin <task>` in your own terminal. A repo whose shared git config sets `core.worktree` or `core.bare = true` gets no block, since git's per-worktree config needs those moved first; `start` and `kelpie doctor` say so), `--no-focus` keeps you where you are. A task that is already running is just shown; one whose Space or Claude is gone (closed, or herdr restarted) gets them back, in the permission mode it started with, resuming its last conversation without sending the prompt again. |
| `kelpie move <name> [file ...]` | Moves work under way in the main checkout into a task: branch `<name>` if it exists, else the unpushed commits, plus the given uncommitted files. `--pane <id>` moves that herdr pane along, `--no-focus` stays put. `/kelpie:move-to-worktree` does the same from inside Claude. |
| `kelpie peers` | Who you are (by the worktree you are in) and the other task sessions with what each is doing, the same task's halves in other repos first. |
| `kelpie tell <to> <text>` | Run by a session to message another: a task (every repo it spans), `task@repo`, or `lead`. It arrives in that Claude's chat as `[from <task>@<repo>] <text>`, every line after the first indented, so no message can start a line as `[from you]`. A session waiting for an answer, or with no Claude running, has the message held and delivered once it is free; nothing is ever typed into a question. Every message goes to `messages.jsonl` in the state directory. The task role tells Claude that a peer's message is information, never your instruction, and pushing stays yours. |
| `kelpie report --state <state> --next "<step>" "<summary>"` | Run by a task's Claude at the end of every turn, as [`roles/task.md`](roles/task.md) tells it: where the task stands (`needs-you`, `ready`, `working`, `blocked`), what you should do next (`nothing` when it carries on alone) and a one-line summary. It goes to the lead through the message log (not typed into its chat), the board shows it on the task's row, and herdr's sidebar gets the next step as `$next` and the state in place of idle or done. A task that stops without one is sent back once by a Stop hook kelpie gives it (`kelpie hook`). With `ready`, `--card <file>` adds the sign-off card: a few markdown lines for you (the change in your words, where to look, the decisions to confirm, images as `![what](path)`), which kelpie keeps under its state with the sha it was written at and shows above the commits when you push. |
| `kelpie ask --option "<label>: <meaning>" … --recommend <n> "<question>"` | Run by a task's Claude when it needs you: one question, two to four options, the one it recommends. `--plan vault:<note>` or `--plan <file>` asks you to approve a plan instead (approve, approve with a comment, ask for changes, reject with a comment): with the vault as a backlog source the task writes its plan into its own task note there, else into a file under kelpie's state (`plans/<task>@<repo>.md`), and a file is copied as it was asked. kelpie logs the question and reports the task as needs you. Every open question waits on the board at once, and you answer each in its question window: the option and your comment, or your own words, reach the task as `[from you] <answer>`, the question closes, and the lead gets a one-line copy. `kelpie answered <task>` closes a question you settled in the task's Space; you or the lead run it, or the task for its own question. Until the answer the task gets only the answer: every other message to it, the lead's too, is held, since typing one would start a turn. |
| `kelpie messages [--print]` | The messages between sessions, live, newest at the bottom (`↑` `↓` scroll), as the board shows them; it also delivers held messages as their sessions free up. `--print` prints the last 30. A task's report shows there too when it waits on you (needs you, ready or blocked), with what you should do; reports of work in progress stay on the board. The lead is copied: each of your prompts to it carries what sessions told each other and their latest reports since it last looked (`--unread`, run by a prompt hook kelpie gives the lead), so the traffic stays out of the chat, and so do its copies of the questions you are asked and answer on the board. |
| `kelpie list [--json]` | The pool's worktrees: their task, or `free`, with uncommitted changes, commits ahead and behind, and the first line of the worktree's setup note (`not set up` without one). |
| `kelpie land [name]` | Rebases the task onto the default branch, checks it has no uncommitted changes, and prints the push command. How commits look is the rules skill's call (see [Rules](#rules)). It never pushes. On a conflict it aborts its rebase, leaving the worktree as it was, and hands the rebase to the task's Claude, which resolves it and reports back. |
| Push (board) | Offered only while the task's Claude is still, and for a task in review only once its reviewer passed it. Shows the task's sign-off card, marked when the code changed since it was written, then the commits that will land, with the selected one's full message. `e` edits that message in the board; `s` squashes them into one, with a message Claude drafts in the background from the commits and the diff, ready to edit; ctrl-s saves, esc goes back, ctrl-o opens the text in your editor. `y` pushes exactly the commits shown and closes the task; a commit that arrived after the dialog opened stops the push. `p` pushes the commits up to the selected one and keeps the task open: its branch continues from the new default branch with the rest on top, an approval of the whole branch holds for the rest, and its Claude is told. Only messages and the squash change; the code stays identical. |
| `kelpie tidy <name> [note]` | Asks the task's Claude to reshape its commits for review following the rules skill (`task.rules_skill`), keeping the final tree identical. The note says how, e.g. "squash into one". |
| `kelpie review [name]` | Sends the task to review (`v` on the board): it waits under To review, and its push with it, until a verdict `ok` on the change HEAD makes (a clean rebase, a new message or a squash keeps a pass, a changed line does not), and when it reports ready on changed code kelpie starts the reviewer again. A task never sent goes straight to push. `--withdraw` (`v` again on the board, yours only: neither the lead nor a task can) takes a task out of review: the reviewer stops, its verdict goes, push is offered again and the lead is told. The reviewer is a separate Claude (`claude -p`) in the task's worktree, with [`kelpie:review`](skills/review/SKILL.md) as its instructions, the commits against the default branch, the prompt and later messages from the task's own conversation, and the rules skill (`task.rules_skill`). It only reads: its tools are Read, Grep, Glob and the shell for `git log`, `git diff` and `git show` alone, in `dontAsk` mode, with a hook that refuses every other command, and none of your own settings, hooks or plugins. Its verdict goes to `verdicts/<task>@<repo>.md` in the state directory: `verdict: ok` or `verdict: changes`, the sha it reviewed, then findings with `file:line`, bugs first. The lead is told `<task>@<repo> reviewed at <sha>: ok \| N findings — <file>`, and the task's row shows `reviewed` or `review: N findings`, `review stale` once its code changes. The pass is the reviewer's, separate from the lead's `kelpie approve`. `review.model` picks its model. |
| `kelpie approve <name>` | Run by the lead once a ready task's commits hold to the rules skill; you may run it too. kelpie reads who runs it from the process tree, not the folder, so a task's Claude does not approve its own work by mistake or because a message tells it to (see [Trust boundary](#trust-boundary)). The board shows the task under Ready to push while its commits stay as approved; a new or changed commit takes the approval back, a clean rebase keeps it. |
| `kelpie push [name]` | After land: pushes the task to the default branch and closes it as `done`; `--through <sha>` pushes its commits up to that one and keeps the task open, like `p` in the board's push dialog. A task in review pushes only code its reviewer passed. You run it (or the board's Push, which shows the card and the commits and asks first). The lead may run it only under your push permission: `a` on a task's row gives it for that task (done, drop and reset take it back), `a` on an area in Areas gives it for every task in that repo whose reviewer passed its current code; either way the lead's rules approval must match the commits. No other Claude pushes: kelpie reads who runs it from the Claude above it. After every push the lead is told which task pushed which commits, and writes the vault log line. |
| `kelpie away [on\|off]` | Away mode, your switch (`A` on the board, which shows it in its header). While it is on, the lead settles a task's question about method itself (`kelpie decide <task> "<answer>"`, recorded as its call), keeps a question about what you want for you (`kelpie defer <task>`, which brings the next question), and pushes only what your permissions allow. `off` hands the lead what was pushed, what it decided and the kept questions since you left, for the summary it writes you. |
| `kelpie done [name]` | Once every change on the branch is on the default branch, pushed as is, cherry-picked or squashed (otherwise it names the commits that are missing): closes the Space, deletes the branch, frees the worktree, and fast-forwards the main checkout when it is clean. When the task was started from a backlog item (or linked to one), the lead is told to log and close it. |
| `kelpie drop [name] [--force]` | Throws the task away; refuses while it has uncommitted changes or unpushed commits unless `--force`. |
| `kelpie link <name> <item>` | Ties a running task to a backlog item or project note (a vault path, a Linear id, or `local:<Title>`), as `start --backlog` does: when the task is done the lead is told to log and close the item, and a reset puts it back in the backlog. The lead runs it when a plain task grows into a project. |
| `kelpie reset [name]` | Stops a task for good (`z` Reset on the board, which asks first): its changes and commits are discarded, its worktree is freed and its Space closes, the lead is told what was thrown away, and the backlog item it was started from shows in the Backlog column again. |

Without a name, `land`, `review`, `push`, `done`, `drop` and `reset` act on the task whose worktree you are in; `-C <dir>` runs any command as if started in `<dir>`.

### Trust boundary

kelpie keeps a task's Claude from pushing, approving, switching away mode or answering for you by accident, or because its prompt, a file it read or another session's message told it to. It is not a sandbox. Every gate reads the process tree and files in your own account, so a hostile process running as you can get past all of them, for example by typing into your shell through herdr. Work on code you do not trust needs OS-level separation: another user account, a container or a VM.

Within that boundary kelpie fails closed: when it cannot read who runs it (`ps` fails, or the chain of parents breaks), or a kelpie runs cut loose from any terminal while a Claude that kelpie started is alive, it treats the caller as a Claude and refuses what only you or the lead may do. The lead pushes only under the push permission you gave it.

Git is the only state: a worktree on a task branch is busy, a clean one that is detached or on its own `wt<N>` branch is free. Worktrees are kept and reused; the setup note each one keeps makes setting it up again for the next task a quick check.

### Rules

How commits look is a skill's call, not kelpie's code. `task.rules_skill` names the skill; kelpie ships [`kelpie:work-rules`](skills/work-rules/SKILL.md) as the default: a subject and a body that says why, one logical change per commit, staging by explicit file list, the push left to you. Your machine config replaces it for every repo, a [repo config](#configuration) for that repo; personal rules go in your own skill. A task's Claude follows it from its first turn, and `tidy` reshapes commits by it. When a task reports `ready`, kelpie asks the lead to check its commits against the skill: the lead tells the task what to fix, or runs `kelpie approve`. Only an approved task shows under Ready to push; until then it waits under Needs you, marked "rules not checked by the lead", and Push works from there too.

### The lead and the board

`prefix+space` takes you to the **lead** Space, and back to where you were on the next press. The first time, it creates it: the **board** on the left, with the messages between sessions, and on the right a Claude started with [`roles/lead.md`](roles/lead.md), the session you talk to most. It is the master of all work: it holds the whole picture, finds work in the configured backlog sources, writes informed task prompts, starts and checks tasks, reviews what they produce, passes on what one task needs to know from another, does small cross-cutting fixes itself and brings you recommendations; feature work happens in each task's own Space, and only you push, or the lead for what you gave it push permission. The lead Space stays at the top of herdr's Spaces (see [What starts by itself](#what-starts-by-itself)). `prefix+tab` (`kelpie focus`, the herdr action `kelpie.focus`) moves focus between the board and the lead's chat, and from any other Space goes to the chat. A lead Space that still has the separate messages pane under the board loses it the next time you go to the lead or restart it.

The lead's Claude reads its role and settings when it starts. `kelpie lead --restart` brings a running lead onto the current ones: it writes the role and settings files again, ends the lead's Claude in its pane (escape, then ctrl-c until the shell is back) and starts it there again with `--resume`, so the conversation goes on under the new role. kelpie records the lead's session id when it starts it and resumes that conversation, or the newest one in the lead's directory when that is newer (after a `/clear`); with none found it starts a new one and says so. It refuses while the lead is working or waiting on a question, so run it when the lead is idle; with no lead running there is nothing to restart. `kelpie update` does this by itself for a waiting lead.

The board (`kelpie board`, also over any Space with `prefix+t`) shows only what herdr's sidebar doesn't. A status line counts tasks that need you, are working, are to review or are ready to push, and the questions waiting for you. **Work** and **Backlog** sit side by side (stacked in a narrow pane; `←` `→` switch). Work lists your tasks grouped by what they need, from every repo with a worktree pool and not only those with a Space open, so a task whose Space is gone shows as `no Space` and `g` restarts it (a task's halves in other repos next to each other), with commits, changed files and how far behind master, and under each a `you:` line with the next step from the task's last report and, while its dev server runs, a `web` line with the server's address (`worktree.dev_url`) as a terminal hyperlink, so it opens from the board over ssh too. Between turns the report decides the group, and Needs you puts open questions first, then what tasks ask of you, then what is ready. The selected one opens in place with review progress and the question it asked through `kelpie ask` with its options (or the question on its screen), its report, or, until it first reports, Claude's recap, and warns about files changed in a main checkout outside any task. Backlog lists the items of the configured sources (the local backlog file, the vault backlog, Linear issues) by priority, and `a` adds one to the local file. `⏎` brings up the options for the selected row (go to its Space, message its Claude, open the review, start or stop its dev server when the repo sets `worktree.dev_server`, start or stop its worker when it sets `worktree.dev_worker`, tidy commits, send it to review when it has commits, push, let the lead push it, done, drop; or start a backlog item as a task), each with a letter that also works directly. A task started from the board starts in the background, so the board stays put and the task shows under Work once its Space is up; while it runs, the backlog item it came from leaves the Backlog column, and a reset brings it back. Vault backlog notes show as soon as they are filed, without waiting for a refresh. The lead keeps the backlog with `kelpie backlog save`; `R` asks it to refresh. The messages between sessions sit at the bottom: `m` shows them on one line, then opens them with the newest selected (`↑` `↓` move through them, `⏎` reads one whole, `esc` goes back to the tasks, `m` on an open panel goes into it), then hides them. `?` lists every key. The mouse works too: click a row to select it and again for its options, click an option or a commit, click the messages line to open it and a message to select it (again to read it), click a key in the status line to press it, scroll to move through what is under the pointer, click outside a window to close it.

`kelpie status [--json]` gives the same picture on the command line, for the lead and for scripts.

### Plan usage

The board's status bar shows how much of your Claude plan's five-hour session and seven-day week you have used, and when each resets. Claude Code passes those numbers only to a statusLine command, so make kelpie yours, in `~/.claude/settings.json`:

```json
"statusLine": { "type": "command", "command": "~/.local/share/kelpie/current/target/release/kelpie statusline" }
```

`kelpie statusline` keeps the windows in `usage.json` in the state directory, with when it saw them, and prints nothing, so Claude Code shows no line of its own under the prompt. The numbers are as fresh as the last response of any Claude session; a window whose reset has passed drops off the bar. Claude Code passes them only on a Claude.ai plan, so with an API key the bar shows none. `kelpie doctor` says whether the line is set.

### The local backlog

With `"local"` among the sources (the default), the Backlog column reads `backlog.md` next to the machine config (`~/.config/kelpie/backlog.md`) as it changes, and you can edit it by hand. One item per line:

```markdown
- [high] Fix login — the session drops on reload (app)
- Write the release notes (web)
```

The priority is `urgent`, `high`, `medium` or `low` (none when left out); the summary after ` — ` and the area in parentheses at the end, the folder name of the repo the work happens in, are optional, but starting a task needs the area. The title is how kelpie knows the item, so keep titles unique. Other lines are yours: kelpie only appends, with `a` in the Backlog column (a title, then a priority) or `kelpie backlog add --priority high "Fix login — the session drops on reload (app)"`. While a task started from an item runs, the item leaves the column; a reset brings it back, and when the task is done the lead deletes its line.

### With the vault

The vault plugin ([notes-vault-mcp](https://github.com/gronare/notes-vault-mcp) in Claude Code) is optional. Without it kelpie keeps its backlog and state as above. With it installed, enabled and configured, kelpie finds it by itself and keeps each task in the vault:

- **Start**: every `kelpie start` (from the lead, the board or the backlog) opens the task's note under `Projects/`, or claims the vault backlog note it was started from, and ties it to the task like `kelpie link`. A task across several repos has one note: each half after the first takes the same one, and done or reset settles it with the last half. The task's role names the note.
- **Edits wait for the Problem**: until the note's Problem section is written, the task's Claude cannot edit files, nor run a shell command that plainly writes one in its worktree (a `>` or `>>` redirect, `tee`, `sed -i`, `cp` or `mv` into it, `git apply`, `patch`). It writes Problem itself, in its own words, from your prompt: your prompt never goes into the vault as you typed it. A vault that does not answer lets the edit through with a warning, so an outage never locks a task out. The gate is a guard rail, not a sandbox: a script that writes files gets past it.
- **Push**: after each push, the user's or the lead's, kelpie writes the repo's log line itself: the commit subjects with their shas.
- **Done** waits until the note has its solution and decisions; kelpie tells the task what is missing, and once they are written it closes the note. **Reset** and **drop** put the note back in the backlog.

kelpie runs the vault through the plugin's own pinned command (read from its hooks), or `[vault] command` in the machine config. A vault call that fails never stops a start, a push or a done: the output says what was not written. `kelpie setup vault` installs the plugin, showing each `claude plugin` command and running it only on your `y`; `kelpie doctor` says whether the vault is on, missing (optional) or too old for the task commands (`claude plugin update vault@gronare`).

### Areas

`tab` switches the board between **Work** and **Areas** (or click the tabs top right). Areas lists the vault's system notes (`Areas/*.md`) with their repo, grouped into needs you (main ahead of origin, commits since the last release, or a task waiting on you), active and quiet: running tasks, open notes and backlog, main against origin, the last tag and commits since, and the last log line. Areas without a repo on this machine still list. The repo is the system note's `path`, else where the vault's hooks saw it, else a checkout named after the area. kelpie reads [notes-vault-mcp](https://github.com/gronare/notes-vault-mcp)'s local index (found through the vault plugin's options in Claude Code, or `VAULT_PATH` / `S3_*` and `VAULT_CACHE_DIR` as the server takes them), so a vault on S3 needs no credentials here; it is as fresh as the last session that used the vault. `kelpie areas` prints the same on the command line.

`⏎` lists an area's actions, and `← →` moves into the detail to pick one; each has a letter that also works directly, and the mouse works as on Work. Built in: open its Space (`g`), start a task in its repo told the area's notes (`s`), ask the lead to refresh its backlog (`b`), its vault log (`l`), push main (`u`, only when main is ahead: shows the commits and pushes as you on `y`, like Push on a task), and let the lead push the repo's reviewed tasks (`a`).

A repo gets its own actions in its [repo config](#configuration), `~/.config/kelpie/repos/<repo>.toml`:

```toml
[[actions]]
key = "R"                     # one character, not one of g s b l u a j k q ?
title = "Release"
about = "bump, commit, tag, push"
ask = "version"               # asked in turn; a version is prefilled with the next patch
bump = ["pyproject.toml", { file = "uv.lock", after = 'name = "notes-vault-mcp"' }]
run = ["uv build"]            # shell commands in the main checkout, after the bump
commit = "{version}"
tag = "v{version}"
push = true                   # asks again, then pushes main and the tag as you
then = ["claude-plugins:bump-pin"]   # repo:action offered once this is done, never run on its own

[[actions]]
id = "bump-pin"               # defaults to the title, slugged
key = "P"
title = "Bump a pin"
ask = ["plugin", "version"]
task = "Bump {plugin}'s pin to {version}; commit."   # starts a kelpie task with a Claude instead
```

A script action runs in the main checkout, which must be on the default branch, clean and not behind origin. The window shows each step first (the exact lines a bump changes, only lines that say version and hold the current one; `after` limits a file to the first such line after a marker), and nothing changes until `y`; pushing asks once more.

## Worktree setup

kelpie creates a bare worktree; the task's Claude makes it runnable as its first step, with the skill `worktree.setup_skill` names. Its role text gives it the worktree's path, its slot, the main checkout, that skill and, when set, `worktree.setup_command`.

`/kelpie:set-up-worktree` is the default. It first reads what the repo says about worktrees: AGENTS.md, CLAUDE.md, README.md, CONTRIBUTING.md and any project skill in `.claude/skills` (in the worktree or the main checkout, through a symlink too) whose name or description is about worktrees. From them it takes what the worktree shares with the main checkout, which stays read-only (a shared development database is used, never migrated from the worktree), how its services start, and its URLs and ports; their steps for creating or removing a worktree do not apply, since kelpie made this one. Then it runs the repo's prepare command, `worktree.setup_command` when set, else `bin/setup-worktree` when there is one; else follows a worktree section in those docs or skills; else works the setup out from `bin/setup`, the lockfiles and CI: dependencies, the gitignored keys and `.env` files copied from the main checkout, the worktree's own databases, a free port, then one fast check. A worktree inherited from a previous task is taken over first, before any route, the prepare command included: databases holding that task's migrations are rebuilt from this tree's schema, and its runtime state inside the worktree (`tmp/`, logs, caches, compiled assets, and uploads along with a rebuilt database) is cleared, while dependencies, copied inputs and the port stay. It asks before anything that deleting the worktree does not undo, and never writes to the main checkout, and adds `[worktree] dev_server` to the repo config on this machine when you say yes. What it did goes in a note at `$(git rev-parse --git-dir)/kelpie-setup.md`, outside `git status`, so the next task in that worktree only checks what has drifted. `kelpie move` and `/kelpie:move-to-worktree` run the same skill.

`bin/setup-worktree` is the repo's own answer: executable, no arguments, idempotent, run from inside a worktree, with no knowledge of kelpie, so a colleague who runs `git worktree add` by hand gets the same result.:

```sh
#!/bin/sh
set -eu
main=$(dirname "$(git rev-parse --path-format=absolute --git-common-dir)")
for f in config/master.key .env; do [ -e "$f" ] || cp "$main/$f" "$f"; done
bundle check >/dev/null || bundle install
bin/rails db:prepare
```

A repo whose own prepare script has another name, or takes arguments, gets `[worktree] setup_command` (`setup_command = "script/prepare_worktree"`), and the skill runs that in place of `bin/setup-worktree`. A command that names a path applies only in a repo that has that file, so a team whose repos share a script name sets it once in the machine config and every other repo keeps the default setup.

`/kelpie:make-worktree-safe` audits a repo for what breaks across worktrees (fixed ports, shared databases and services, gitignored inputs, absolute paths, shared caches) and, when you say yes, writes `bin/setup-worktree`, the port fix and a `## Working in a worktree` section in a worktree for you to review. You run it; Claude never starts it on its own.

## Review split

`/kelpie:review-split [base or what to review]` opens a split with the files of the work under review. Claude works out the scope first: the files it changed in this session, or, for work from elsewhere, what the unpushed commits, uncommitted changes and any project notes say belongs to it, asking when that is unclear. The list shows those files in the order of Claude's numbered review guide; other changes in the repo are one key away. Ask for everything and you get every change against the base. Inside a git worktree, the review is based on where its branch left the remote's default branch, so it shows exactly that worktree's work, committed or not, however far the default branch has moved on.

| Where | Key | What it does |
| --- | --- | --- |
| list | type | filter; `!spec/ ` is prefilled to hide specs |
| list | Enter | open the file's unified diff in nvim |
| list | ctrl-r | mark the file read, or unread |
| list | ctrl-h | show or hide read files |
| list | ctrl-o | show or hide changes outside the work under review |
| list | ? | show every key in the preview pane |
| list | ctrl-c | close the split (Esc does nothing, so it cannot close it by accident) |
| diff | c | comment on the line or visual selection |
| diff | o | open the real file over the diff, at that line |
| diff | - | open the file's folder over the diff (neo-tree's float if installed, else netrw) |
| diff | u | reload the diff after the file changed |
| diff | w | toggle line wrap |
| diff | r | mark the file read and go back to the list |
| diff | q, Esc | back to the list |
| opened file | leader m | comment on the line or visual selection |
| opened file | ctrl-q | back to the diff |

Files opened with `o` or `-` show in a float over the diff, and everything else in it is your usual nvim: jump around, search, edit. When you go back to the diff, files you saved are reported to Claude as `path:lines — edited by me during the review`, and unsaved edits ask to be saved or discarded first.

A comment is typed into Claude's input box as `path:line — text`, not submitted, so several pile up until you press Enter there. A read file stays out of the list until its contents change; marks live in the state directory, one file per repo.

The list follows the repo by itself: it reloads when files change, keeping your filter and cursor. An open diff marks itself `changed on disk` instead of moving under you; `u` reloads it.

With **Open the review pane automatically** on (see [What starts by itself](#what-starts-by-itself)), the first file edit by a Claude that runs in a git worktree (a task) opens the review split next to it without taking focus; a session in a main checkout or outside any repo never opens one on its own. Everything since its branch left the default branch is in scope; where kelpie cannot find that point, files already changed before that edit count as other changes (behind ctrl-o) and everything changed after it is in scope, newest first. Close the pane from herdr and it stays closed for that session; `/kelpie:review-split` brings it back, or rescopes the open one to Claude's guide.

`/kelpie:history-split` opens tig in a split for the branch's history.

## What starts by itself

Everything kelpie runs without being asked, and how to turn it on or off. `herdr plugin disable kelpie` stops all of the herdr side at once.

| What | When | On or off |
| --- | --- | --- |
| The `current` link (`scripts/herdr-startup.sh`) | Every herdr start: links `~/.local/share/kelpie/current` to the installed plugin. | Always on with the herdr plugin; the PATH link and the hooks rely on it. |
| The lead Space kept first | Whenever a Space is created, moved or reordered, herdr runs `kelpie lead --first`, which moves the lead Space back to the top. Without a lead Space it does nothing. | Always on with the herdr plugin. |
| The sidebar sync (`scripts/sync.sh`) | Every 3 seconds while it is in herdr's tab bar: a `server` tag while the dev server the board's `w` started runs in the Space and a `worker` tag while the worker its `b` started runs, Claude's task title on its pane border, model and context in use on its Agents row, and tab names. It prints nothing. | Off until you add it to `[ui] tab_bar_right` (below); remove the line to stop it. |
| The board's fetch | While a board is open, it fetches each repo it shows (tasks' main checkouts and the Areas page's repos) every 5 minutes in the background, one fetch per repo at a time, and names a repo whose fetch failed in its header. | Runs only while a board is open. |
| The board taking over a rebuild | An open board restarts itself on a newly built `kelpie` binary, keeping its column and selection. | Runs only while a board is open. |
| Held messages | An open board delivers messages held for a busy session once it is free, and so does `kelpie messages`. | Runs while a board is open. |
| A task's hooks | kelpie starts every task's Claude with a prompt hook and a Stop hook (`kelpie hook`): a turn that ends without `kelpie report` is sent back once to report. The same settings deny `git push`, `kelpie approve` and `kelpie push` to it. | Always on for tasks. |
| The lead's prompt hook | Each of your prompts to the lead carries the sessions' messages and reports it has not seen (`kelpie messages --unread`). | Always on for the lead. |
| The reviewer again | A task in review that reports ready on code its verdict no longer covers is reviewed again. | Only for tasks sent to review; `kelpie review --withdraw` takes one out. |
| Live review | The Claude Code plugin's edit hook opens the review split on a task's first file edit. | Off by default. Turn it on in Claude Code under `/plugin` → Installed → kelpie → Configure, or at install with `claude plugin install kelpie@kelpie --config auto_open=true`. |
| Notifications | `notify.command` runs when a task needs you or is ready, when a task asks you a question, and on the lead's `kelpie notify`. | Off until `[notify] command` is set ([Notifications](#notifications)). |

The sidebar sync in herdr's `config.toml`:

```toml
[ui]
tab_bar_right = [
  { type = "command", command = "~/.local/share/kelpie/current/scripts/sync.sh", interval_seconds = 3, timeout_seconds = 3 },
]
```

## Configuration

kelpie reads its settings from this machine only, never from a file inside a repo, so a repo carries nothing that applies to some of the people who clone it. Three layers, read key by key; the first that sets a key wins:

1. The repo config, `~/.config/kelpie/repos/<repo>.toml` (`KELPIE_REPOS_DIR` names another folder), where `<repo>` is the main checkout's folder name, so every worktree of `app` reads `repos/app.toml`. It holds `[worktree]`, `[task]`, `[review]` and the repo's [actions](#areas).
2. The machine config, `~/.config/kelpie/config.toml` (`$XDG_CONFIG_HOME/kelpie/config.toml`; `KELPIE_CONFIG` names another file).
3. kelpie's built-in defaults.

Either file may be a symlink, and kelpie follows it. `kelpie config` and `kelpie doctor` show a link together with its target. [`kelpie:configure`](skills/configure/SKILL.md) is the skill a Claude follows to change a setting or set a machine up, and it verifies each change with `kelpie doctor`.

A team shares its setup through a team repo: the repo configs live there, with one short file that lists which file links to which kelpie path, and each developer clones it and links the files in:

```sh
ln -s ~/projects/dotclaude/kelpie/repos/app.toml ~/.config/kelpie/repos/app.toml
```

A pull in the team repo updates every linked setting; asking Claude to adopt the team's kelpie setup runs `kelpie:configure`, which finds that file, makes the links and checks them.

A key that applies to every repo (`[board]`, `[notify]`, `[update]`, `[vault]`, `server_name`, `worker_name`) lives in config.toml; in a repo config it is an error that says so. An unknown key in either file is an error that names the file and the key. A path in a repo config (an action's `bump` file) stays inside the repo, and a skill name (`setup_skill`, `rules_skill`) is a name, never a path.

`kelpie config [--repo <path>]` prints the files it found, then every key with its value in effect, the layer it comes from (`repo config`, `machine config` or `default`) and what it does. Inside a repo it reads that repo's file; `--repo` names another. `kelpie doctor` checks config.toml and every file in the repos folder, a broken link included.

| Key | Where | Default | What it does |
| --- | --- | --- | --- |
| `[board.backlog] sources` | machine config | `["local"]` | Where the board's Backlog tab finds work, in any combination: `"local"`, the [local backlog file](#the-local-backlog); `"vault"`, the vault's backlog notes; `"linear"`, your open Linear issues, read by the lead through a Linear MCP server. The first one listed is where the lead files new ideas. `[]` turns the backlog off. Example: `sources = ["vault", "linear"]`. |
| `[worktree] setup_skill` | either | `"kelpie:set-up-worktree"` | The skill a task's Claude runs first to make its worktree runnable: `plugin:skill` for a plugin's skill, a bare name for one in `~/.claude/skills` or the repo's `.claude/skills`. Example: `setup_skill = "my-tools:set-up-worktree"`. |
| `[worktree] setup_command` | either | none | The repo's own shell command that prepares an existing worktree, run from inside it; the setup skill runs it, and a task's role text names it. A command whose program is a path applies only in a repo that has that file; elsewhere, and when none is set, `bin/setup-worktree` is used when the repo has one, else the setup skill's own routes. `kelpie doctor` warns when a repo config names a path that is missing or not executable. Example: `setup_command = "script/prepare_worktree"`. |
| `[worktree] dev_server` | either; usually the repo config | none | The shell command the board's `w` runs in a `server` tab of the task's Space, and `w` again stops with ctrl-c. `w` exports `PORT` as the worktree's own port, the one set-up-worktree recorded (`PORT` in the worktree's `.env`, else `port <n>` in its setup note), and sets none when neither has one. Without it the board has no `w`. In the machine config it applies to every repo that sets none. Example: `dev_server = "bin/dev"`. |
| `[worktree] dev_worker` | either; usually the repo config | none | A second command, such as a job runner, that the board's `b` runs in a `worker` tab of the task's Space and `b` again stops with ctrl-c, with the same `PORT` as `w`. Without it the board has no `b`. Example: `dev_worker = "bin/jobs"`. |
| `[worktree] dev_url` | either; usually machine config | `"http://localhost:{port}"` | The address of a task's running dev server, which the board shows under the task as a link and `w` names when it starts the server; `kelpie status --json` carries it as `url`. The address depends on where the servers run, so it is a template: `{worktree}` is the worktree's folder (`wt1.app`), `{repo}` the repo's (`app`), `{number}` its pool number (`1`) and `{port}` its port, read like `w`'s `PORT`. A worktree without what the template names (no recorded port for `{port}`) gets no link. `""` turns the link off. Any other `{…}` is an error. Examples: `dev_url = "http://localhost:{port}"` when the servers run on this machine; `dev_url = "https://{worktree}.dev"` behind a proxy that serves each checkout under its folder name, such as puma-dev. |
| `[worktree] server_name` | machine config | `"server"` | The name of the tab the board's `w` starts `dev_server` in, and the value of the Space's `$server` tag in herdr's sidebar while it runs. Machine config only, since the sidebar names every Space alike. Example: `server_name = "web"`. |
| `[worktree] worker_name` | machine config | `"worker"` | The same for `b`'s worker tab and the `$worker` tag. Example: `worker_name = "jobs"`. |
| `[task] rules_skill` | either | `"kelpie:work-rules"` | The skill with your rules for how work is planned, built and committed (a mock before a new screen, commit style, what never to do). A task's Claude runs it after the setup skill and follows it for the rest of the task, the lead checks a ready task against it, `tidy` reshapes commits by it, and the reviewer reads it. Named like `setup_skill`. Example: `rules_skill = "my-rules"`. |
| `[review] model` | either | none | The model `kelpie review`'s reviewer runs on (`--model`); none leaves it to Claude's default. Example: `model = "opus"`. |
| `[notify] command` | machine config | none | The shell command kelpie runs when something needs you; see [Notifications](#notifications). None sends nothing. |
| `[update] channel` | machine config | `"release"` | What `kelpie update` installs on this machine: `"release"`, the highest `vX.Y.Z` tag on origin; `"main"`, origin's default branch as it is pushed. See [Update](#update). Example: `channel = "main"` on the machine you test new code on. |
| `[vault] command` | machine config | none | The notes-vault-mcp command kelpie writes task notes and log lines with, before its own arguments; none runs the vault plugin's pinned command from its hooks. Used only while the plugin is installed and enabled; see [With the vault](#with-the-vault). Example: `command = "uv run --project ~/src/notes-vault-mcp notes-vault-mcp"`. |

A machine config:

```toml
[board.backlog]
sources = ["vault", "linear"]

[worktree]
setup_skill = "my-tools:set-up-worktree"
dev_url = "https://{worktree}.dev"

[task]
rules_skill = "my-rules"

[review]
model = "opus"
```

A repo config, `~/.config/kelpie/repos/app.toml`, next to its actions:

```toml
[worktree]
dev_server = "bin/dev"
dev_worker = "bin/jobs"
```

### Environment

| Variable | Default | What it does |
| --- | --- | --- |
| `KELPIE_CONFIG` | `$XDG_CONFIG_HOME/kelpie/config.toml`, else `~/.config/kelpie/config.toml` | The machine config file. The local backlog, `backlog.md`, is read next to it. |
| `KELPIE_STATE_DIR` | `$XDG_STATE_HOME/kelpie`, else `~/.local/state/kelpie` | kelpie's state: messages, reports, the saved backlog, roles, verdicts, review marks, `notify.log`. kelpie keeps it private: the folder is `0700` and its files `0600`, each replaced whole. Point it at a temp dir for a throwaway probe. `kelpie doctor` lists the directory in use. |
| `KELPIE_REPO_DIRS` | `~/projects:~/personal:~/src:~/code` | Colon-separated folders where `start --repo` looks for checkouts, in them or one level down. |
| `KELPIE_LEAD_DIR` | `lead` in the state directory | The lead Claude's working directory (Claude Code never remembers trust for the home directory, so the lead gets its own). |
| `KELPIE_REPOS_DIR` | `$XDG_CONFIG_HOME/kelpie/repos`, else `~/.config/kelpie/repos` | The folder of repo configs, one `<repo>.toml` per repo. |
| `KELPIE_BIN` | `~/.local/share/kelpie/current/target/release/kelpie` | The binary the lead's role and kelpie's skills run. |
| `XDG_DATA_HOME` | `~/.local/share` | Where herdr's startup writes the `current` link; kelpie reads it under `~/.local/share`, so `kelpie doctor` warns when this is set elsewhere. |
| `CLAUDE_CONFIG_DIR` | `~/.claude` | Claude Code's config, where kelpie and `kelpie doctor` read the installed plugins, their settings and the vault plugin's options. |
| `KELPIE_VAULT_BIN` | none | A vault command to use in place of the plugin's, for a probe or a test: it stands in for the plugin, so kelpie asks it `status` and uses it when it answers configured. |
| `KELPIE_CLAUDE_BIN` | `claude` | The Claude Code binary `kelpie setup vault` runs. |
| `VAULT_PATH`, `S3_ENDPOINT`, `S3_BUCKET`, `S3_PREFIX`, `VAULT_CACHE_DIR` | the vault plugin's options | Which notes-vault-mcp index the Areas page and the vault backlog read, as the server takes them; the index lives under `~/.cache/notes-vault-mcp` by default. |

`KELPIE_BOARD_STATE` and `KELPIE_PUSH_TOKEN` are kelpie's own handoffs between its processes; never set them.

### Notifications

kelpie has no notification service of its own: `[notify] command` names a shell command, and kelpie runs it when something needs you. That is when a task reports `needs-you` or `ready`, when a task asks you a question (`kelpie ask`), and when the lead (or you) runs `kelpie notify "<text>"`, which no task can, for instance with its summary while you are away. The command runs through `sh -c`, detached, so the report never waits on it, and is stopped after 10 seconds. It gets the event in its environment:

| Variable | Value |
| --- | --- |
| `KELPIE_EVENT` | `needs-you`, `ready`, `question` or `summary` |
| `KELPIE_TASK` | the task's name; empty for `summary` |
| `KELPIE_REPO` | the repo's folder name; empty for `summary` |
| `KELPIE_TEXT` | one line for you, e.g. `billing@app needs you: answer which queue` |

The text is also `$1`, so a script can take it as an argument: `command = "~/bin/notify \"$1\""`. A task gets at most one notification per event a minute. A command that fails or times out never fails the report: its error goes to `notify.log` in the state directory. `kelpie doctor` says whether the key is set and never runs it.

To your phone through [ntfy](https://ntfy.sh), in the machine config:

```toml
[notify]
command = 'curl -s -H "Title: kelpie $KELPIE_EVENT" -d "$KELPIE_TEXT" https://ntfy.sh/my-kelpie-topic'
```

As a macOS notification:

```toml
[notify]
command = 'osascript -e "display notification (system attribute \"KELPIE_TEXT\") with title \"kelpie\""'
```

## License

MIT
