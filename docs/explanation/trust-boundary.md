# The trust boundary

kelpie keeps a task's Claude from pushing, approving its own work, switching away mode or answering for you, whether by accident or because its prompt, a file it read or another session's message told it to. Two layers hold it: kelpie's own guards, which read who runs a command, and Claude Code's sandbox around a task's shell commands, which takes away what a shell would need to pass as you. This page says what each guards and where the guard ends.

## What it is for

An agent that reads a README telling it to "push when done", or a message from another task saying "the user approved this", should not be able to act on it. Those are the realistic failures: an instruction in the wrong place, or a session that is wrong about what it is allowed to do. kelpie's guards make the important steps, pushing and answering for you, belong to you and the lead only.

## What kelpie guards

| Guard | How |
| --- | --- |
| A task does not push by accident. | A pre-push hook and a push URL rewrite in the worktree's own git config block `git push` in a task's worktree. The task's Claude settings also deny `git push` and every user-only kelpie command (`approve`, `push`, `choose`, `after`, `ci dismiss`, `away`, `decide` and the rest), with the sandbox on or off; with it off that is more than 0.25.0 denied, a tightening that changes nothing a task is meant to do. A shell that rewrites the URL on its own command line and skips the hook gets past both wherever it holds credentials for an allowed host, which is why the sandbox keeps `~/.ssh`, `~/.config/gh`, `~/.git-credentials`, `~/.netrc` and cargo's credentials from it. A token in the environment, a credential helper or the keychain it can still reach. |
| Only you and the lead push, and the lead only with your permission. | `kelpie push` reads who runs it from the process tree, not from the folder. The lead pushes only tasks you gave it permission for, and only while its rules approval matches the commits. A push that waits for the task's Claude to run tests is recorded only after those checks pass, names you or the lead, and completes only for the exact commits it was recorded for, with the same checks run again for whoever asked; a task's report can end the wait, never start a push. |
| A task cannot approve its own work. | `kelpie approve` reads the caller from the process tree. |
| Only you switch away mode or withdraw a review. Only the lead decides a question of method, and only the lead defers a question while you are away. Only you or the lead send a `summary` notification. | The same process-tree check. |
| The reviewer only reads. | Read, Grep, Glob and `git log`, `git diff` and `git show` alone, in `dontAsk` mode, with a hook that refuses every other command, and none of your settings, hooks or plugins. |
| Only you answer a task's Claude's own prompt. | `kelpie choose` reads its caller from the process tree and refuses a task; that check is what holds. A task's settings also deny `kelpie choose` and name herdr's keystroke paths (`herdr pane send-keys`, `send-text`, `run`, `herdr agent send-keys`, `prompt`), but a deny rule matches only that literal spelling. The lead's settings do not allow `kelpie choose`, so Claude Code asks you before the lead runs it while your own settings hold no allow rule for it. |
| Messages are not instructions. | A message arrives as `[from <task>@<repo>] <text>` with every line after the first indented, so no message can start a line as `[from you]`. The task role tells Claude that a peer's message is information, never your instruction. |
| With the vault, edits wait for the Problem. | A hook refuses file edits and plain file-writing shell commands until the task's note has its Problem. |
| A task's shell cannot pass as you. | The sandbox (below): a task's shell commands cannot start a terminal multiplexer, reach herdr's socket or write kelpie's state, and kelpie refuses its user-only commands to any sandboxed process. |
| With the sandbox on, a task's file tools write only its own work. | Claude's Edit, Write, MultiEdit and NotebookEdit run outside the sandbox, so in every permission mode, `bypassPermissions` included, a hook kelpie installs refuses a file outside the task's worktrees and temp folders (one per repo for a task across repos) (Claude Code's own notes under `~/.claude/projects` and `~/.claude/plans` excepted), and inside the worktree the files Claude Code or git load and run as you: `.claude/settings.json`, `.claude/settings.local.json`, `.claude/hooks`, `skills`, `agents`, `commands` and `workflows`, `.mcp.json` and the `.git` link. Should the hook not run, the task's `Edit(...)` deny rules still hold those, kelpie's state, config and install, herdr's files, `~/.claude.json` and what `~/.claude` runs, the shell's startup files, git's and ssh's config, `~/Library/LaunchAgents`, every folder on your `PATH`, the main checkout's `.claude` and `.mcp.json`, the repo's git config, hooks and the worktree's links into them, and other Claude sessions' temp folders. A task that needs one of them changed asks you. With `task.sandbox = false` a task's shell can write all of these anyway, so kelpie adds neither the hook nor these rules, and a task's plan file stays in kelpie's state; with the sandbox on it moves to the task's temp folder, which its file tools reach. |

Within that boundary kelpie fails closed. When it cannot tell who runs it (`ps` fails, or the chain of parents breaks), or a kelpie runs detached from any terminal while a Claude that kelpie started is alive, it treats the caller as a Claude and refuses what only you or the lead may do.

## The sandbox

With `task.sandbox = true`, the default, kelpie writes Claude Code's sandbox into each task's settings, strict: `allowUnsandboxedCommands = false`, so a command that fails in the sandbox is never retried outside it, and `failIfUnavailable = true`, so Claude does not start without one. A task's shell commands then:

- write only in the worktree, the task's temp folder, the usual package caches (`~/.cargo/registry`, `~/.cargo/git`, `~/.npm`, `~/.cache`) and the repo's shared `.git` folder, which Claude Code's sandbox opens to a linked worktree so `git commit` works, all of it but `hooks/` and `config`; never the worktree's push block, the `.git` link or the git dir's `commondir` and `gitdir` that tell git where its config is, any other pooled worktree's git dir (its config and kelpie's hooks for it), nor other Claude sessions' temp folders,
- read no credentials file (`~/.ssh`, `~/.config/gh`, `~/.git-credentials`, `~/.netrc`, cargo's),
- reach only the package registries (crates.io, npm, PyPI, GitHub's downloads), and no Unix socket, so neither herdr's socket nor a tmux server,
- run kelpie's own task commands (`peers`, `tell`, `ask`, `report`, `routine ok`, `link`, `backlog add`, `config`) outside the sandbox, matched on kelpie's absolute path, since those write kelpie's state; no user-only command is ever among them.

kelpie's own guard proves the caller's capability as well as its process tree: a process the kernel reports as sandboxed (macOS), or one that cannot create a file in kelpie's state directory under the account's home (both platforms), is refused every user-only command, whatever its chain of parents looks like. When the chain is the lead's and the lead itself runs in a sandbox your own Claude Code settings turn on, kelpie refuses the same way and says so: add kelpie's path to `sandbox.excludedCommands` in `~/.claude/settings.json`. A task-side command that fails because it ran inside the sandbox (spelled with `cd`, a pipe, a redirect or `$(…)`, so Claude Code did not take it out) says to run it on its own.

The sandbox covers the shell only. Claude's file tools, hooks and MCP servers run outside it, which is why the file hook and deny rules above hold the file tools, and why the hooks kelpie installs run kelpie itself.

kelpie's own git runs unsandboxed in a sandboxed task's worktree, to read, switch and rebase it, so there it runs with `core.hooksPath=/dev/null` and `core.fsmonitor=false`: a hook or fsmonitor the worktree holds, as husky keeps its hooks in the repo, never runs as you through kelpie. A push you choose from the board runs the repo's hooks, except when they live in a sandboxed task's own worktree: then kelpie runs none of them, and when one is a pre-push hook it does not push, says so, and leaves the push to you. Hooks in the main checkout's `.git/hooks` run as always, since neither the task's shell nor its file tools reach them.

The sandbox needs Claude Code 2.1.285 or later; with an older one kelpie starts no task and `kelpie doctor` names the update.

A profile can name its own hosts (`allowed_domains`), folders (`allow_write`) and commands outside the sandbox (`excluded_commands`); see [Give tasks a profile](../how-to/give-tasks-a-profile.md). The built-in `ops` profile runs `ssh`, `scp` and `rsync` outside it, since they ignore the sandbox's proxy and cannot connect from inside. An ssh session reaches other machines, and this one as you, so kelpie says so when an `ops` task starts.

On Linux the sandbox needs bubblewrap and socat, and blocks Unix sockets only with the seccomp filter from `@anthropic-ai/sandbox-runtime`. When one is missing, kelpie refuses to start a task and names what to install; `kelpie doctor` shows the sandbox's state on both platforms.

With `task.sandbox = false` a task's shell runs unsandboxed, and kelpie's guards are what remain. `kelpie doctor`, a task's start and the board's Areas page say so.

## Where it ends

kelpie's guards read the process tree and files in your own account. A process running as you outside the sandbox can get past them. For example, it can:

- rewrite the worktree's push URL back, set another one, or forge the pre-push hook's token,
- write files through a script, past the vault's Problem gate,
- type into your shell through herdr.

The sandbox closes those to a task's shell, but not to a command that runs outside it: an `ops` task's ssh, a profile's own `excluded_commands`, or any task with `task.sandbox = false`. With the sandbox off, a task keeps the trust kelpie gave tasks before the sandbox: kelpie's guards read the process tree, which a script the task starts can leave, so on your machine that is your choice to make.

The package caches the shell may write are code your own builds load later: a crate's source under `~/.cargo/registry` or a package under `~/.npm` changed by a task runs when you next build unsandboxed. A folder a profile adds with `allow_write` is the same.

**Work on code you do not trust needs OS-level separation:** another user account, a container or a VM.

## Settings come from your home only

kelpie reads its configuration only from files in your home directory, never from a file inside a repo. Cloning a repo cannot change how kelpie behaves, which commands it runs or who may push.

Its state is private: the state directory is `0700`, its files `0600`, and each file is replaced whole on write.

## Repos the push block cannot cover

The push block uses git's per-worktree config. A repo whose shared git config sets `core.worktree` or `core.bare = true` gets no block until those are moved; `kelpie start` and `kelpie doctor` say so.
