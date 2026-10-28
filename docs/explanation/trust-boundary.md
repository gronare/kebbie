# The trust boundary

This page explains what keeps a task's Claude from pushing, approving its own work, switching away mode or answering for you, and where that protection ends.

The protection holds whether the Claude acts by accident or because its prompt, a file it read or another session's message told it to. Two layers hold it:

- kebbie's own guards, which read who runs a command,
- Claude Code's sandbox around a task's shell commands, which takes away what a shell would need to pass as you.

## Instructions in the wrong place cannot act

An agent that reads a README telling it to "push when done", or a message from another task saying "the user approved this", should not be able to act on it. Those are the realistic failures: an instruction in the wrong place, or a session that is wrong about what it is allowed to do. kebbie's guards make the important steps, pushing and answering for you, belong to you and the lead only.

## A task does not push

**A task does not push by accident.** A pre-push hook and a push URL rewrite in the worktree's own git config block `git push` in a task's worktree. The rewrite blocks every network URL, https, ssh and `host:path` alike, and leaves a local path or `file://` URL alone, so a test that makes a repo on the disk and pushes to it still works; the pre-push hook still asks for kebbie's token in the worktree itself. The task's Claude settings also deny `git push` and every user-only kebbie command (`approve`, `push`, `choose`, `after`, `ci dismiss`, `away`, `decide` and the rest), with the sandbox on or off.

A shell that rewrites the URL on its own command line and skips the hook gets past both wherever it holds credentials for an allowed host. That is why the sandbox keeps `~/.ssh`, `~/.config/gh`, `~/.git-credentials`, `~/.netrc` and cargo's credentials from it. A token in the environment, a credential helper or the keychain it can still reach.

## Only you and the lead push

`kebbie push` reads who runs it from the process tree, not from the folder. The lead pushes only tasks you gave it permission for, and only while a rules approval, the reviewer's or its own, matches the commits.

A push that waits for the task's Claude to run tests:

- is recorded only after those checks pass, and names you or the lead,
- completes only for the exact commits it was recorded for, with the same checks run again for whoever asked.

A task's report can end the wait, never start a push.

## Only you and the lead decide

The same process-tree check decides who may:

- approve a task's work: never the task itself, since `kebbie approve` reads the caller from the process tree,
- switch away mode or withdraw a review: you only,
- decide a question of method, or defer a question while you are away: the lead only,
- send a `summary` notification: you or the lead.

## A task cannot approve its own rules check

A ready report only asks for the check. The keeper, or the board while the keeper is off, starts the rules reviewer from its own environment, never the task's, with claude by an absolute path kebbie resolves. It hands the reviewer a nonce on stdin; kebbie's state holds only the nonce's hash.

A reviewer without that nonce, or with a task in its process chain, approves nothing. With the sandbox off, a task's shell can read and write kebbie's state, and the limits named under [Where it ends](#where-it-ends) for a sandbox-off task apply.

## Only kebbie's recheck closes a kebbie issue

After an update, only kebbie's own recheck closes a kebbie issue as fixed. The keeper starts the checker the same way it starts the rules reviewer: its own environment, claude by an absolute path, a nonce on stdin with only its hash kept.

- A task's Claude is denied `kebbie issue recheck` and `kebbie review --run`.
- A checker with a task in its process chain is refused before it reads the keeper's record. A checker without the nonce closes nothing.
- It closes an issue only for a commit that sits after the issue's build, which the install's history must place, and in the code it checked. Anything else leaves the issue open.
- Reports from other installs are never rechecked or closed.

## Only you answer a task's Claude's own prompt

`kebbie choose` reads its caller from the process tree and refuses a task; that check is what holds. A task's settings also deny `kebbie choose` and name herdr's keystroke paths (`herdr pane send-keys`, `send-text`, `run`, `herdr agent send-keys`, `prompt`), but a deny rule matches only that literal spelling.

The lead's settings do not allow `kebbie choose`, so Claude Code asks you before the lead runs it while your own settings hold no allow rule for it.

## Reviewers read, messages inform, edits wait

**The reviewer only reads.** It gets Read, Grep, Glob and `git log`, `git diff` and `git show` alone, in `dontAsk` mode, with a hook that refuses every other command, and none of your settings, hooks or plugins.

**Messages are not instructions.** A message arrives as `[from <task>@<repo>] <text>` with every line after the first indented, so no message can start a line as `[from you]`. The task role tells Claude that a peer's message is information, never your instruction.

**With the vault, edits wait for the Problem.** A hook refuses file edits and plain file-writing shell commands until the task's note has its Problem.

**A task's shell cannot pass as you.** In [the sandbox](#the-sandbox), a task's shell commands cannot start a terminal multiplexer, reach herdr's socket or write kebbie's state, and kebbie refuses its user-only commands to any sandboxed process.

## A task's file tools write only its own work

With the sandbox on, Claude's Edit, Write, MultiEdit and NotebookEdit still run outside it. So in every permission mode, `bypassPermissions` included, a hook kebbie installs refuses:

- a file outside the task's worktrees and temp folders (one per repo for a task across repos), except Claude Code's own notes under `~/.claude/projects` and `~/.claude/plans`,
- inside the worktree, the files Claude Code or git load and run as you: `.claude/settings.json`, `.claude/settings.local.json`, `.claude/hooks`, `skills`, `agents`, `commands` and `workflows`, `.mcp.json` and the `.git` link.

A task that needs one of them changed asks you. With the sandbox on, a task's plan file moves to the task's temp folder, which its file tools reach.

## Deny rules hold if the hook does not run

Should the hook not run, the task's `Edit(...)` deny rules still hold those files, and also:

- kebbie's state, config and install, and herdr's files,
- `~/.claude.json` and what `~/.claude` runs,
- the shell's startup files, git's and ssh's config, `~/Library/LaunchAgents`, and every folder on your `PATH`,
- the main checkout's `.claude` and `.mcp.json`, the repo's git config, hooks and the worktree's links into them,
- other Claude sessions' temp folders.

With `task.sandbox = false` a task's shell can write all of these anyway, so kebbie adds neither the hook nor these rules, and a task's plan file stays in kebbie's state.

## kebbie fails closed

Within that boundary kebbie fails closed. It treats the caller as a Claude, and refuses what only you or the lead may do, when:

- it cannot tell who runs it (`ps` fails, or the chain of parents breaks),
- a kebbie runs detached from any terminal while a Claude that kebbie started is alive.

## Only counts and reports you approve reach the maintainers

Two things leave this machine for kebbie's maintainers, both to kebbie's issues endpoint, `https://issues.kebbie.dev`, both signed with this install's key. Each signal batch and each report sent is a line under messages on the board.

**Error signals**, on by default, are counts of kebbie's own failures, at most once a day. A signal holds only:

- kebbie's version and commit,
- the OS and architecture,
- a component name from kebbie's code, such as `keeper.push`,
- an error kind: `panic`, `internal-error` or `corrupt-state`,
- a count.

## A signal cannot carry your work

The format has no text field, every value is limited to lowercase letters, digits and `.` `_` `+` `-`, and the endpoint refuses a field it does not know. So no message, path or name from your work can travel in one. A panic is named after the kebbie module it happened in, never its message; a panic outside kebbie's code is `dependency`, as its location could hold a path. `[report] signals = false` stops them.

## Only you send a bug report

**Bug reports** are the redacted text of a bug in kebbie you ran into, saved by you or the lead. kebbie asks each time unless you set `[report] text = "always"`. Only you send one: the lead, tasks and sandboxed processes are refused.

`"always"` in config.toml is not enough, since the lead can write that file. It takes effect only once you arm it with `kebbie issue report --always`, a command kebbie refuses to every Claude. Armed, it is your consent in advance: kebbie sends, redacted, what the lead wrote without you reading it.

A Claude that writes kebbie's state directory itself, as the lead's or a sandbox-off task's shell can, gets past this as it does every user-only decision kept there (see [Where it ends](#where-it-ends)).

## Nothing goes to GitHub by itself

`kebbie update` only reads kebbie's GitHub repo, through your `gh`, to find the issues you opened there. An issue reaches GitHub only when you run `kebbie issue send <id>` and press Submit in the browser. kebbie refuses that command, and `kebbie issue link`, to the lead, tasks and sandboxed processes.

## What goes to the hub

A [hub](../how-to/see-your-board-on-your-phone.md) you join is a relay, not a party you have to trust with your work.

- The machine seals each board it posts to your account's key and signs it with its own key, so the hub carries the board without reading it. Only your passkeys unlock the account's key, in your browser.
- The machine pins that key when you connect it from its link. The browser commits to the key with a secret the link carries past the hub, and the machine pins it only when the commitment matches, so a hub cannot slip in a key of its own.
## The board holds no local detail

- The board holds what the protocol lists: task and repo names, summaries, questions and their options, git and CI state. Never a local path, a pane, a Space, a host name or a dev server address.
- The phone only reads: kebbie applies no command from the hub, so nothing the hub or a phone sends starts, answers or pushes anything on the machine.

## What the hub does see

The hub sees the machine's name, which account it belongs to and when it posts. Nothing is pushed to a phone through the hub: notifications through it are not available in this version.

The machine's token goes with every call. So kebbie talks to a hub on another machine over https only, never follows a redirect to another host by itself, and moves to a new address only on your `kebbie hub move`. Connecting, joining, moving and leaving are yours alone; kebbie refuses them to the lead and to tasks.

## The sandbox

With `task.sandbox = true`, the default, kebbie writes Claude Code's sandbox into each task's settings, strict: `allowUnsandboxedCommands = false`, so a command that fails in the sandbox is never retried outside it, and `failIfUnavailable = true`, so Claude does not start without one. A task's shell commands then:

- write only in the worktree, the task's temp folder, the usual package caches (`~/.cargo/registry`, `~/.cargo/git`, `~/.npm`, `~/.cache`) and the repo's shared `.git` folder, which Claude Code's sandbox opens to a linked worktree so `git commit` works, all of it but `hooks/` and `config`,
- never write the worktree's push block, the `.git` link or the git dir's `commondir` and `gitdir` that tell git where its config is, any other pooled worktree's git dir (its config and kebbie's hooks for it), nor other Claude sessions' temp folders.

## A sandboxed shell reaches no credentials or sockets

A task's shell commands in the sandbox:

- read no credentials file (`~/.ssh`, `~/.config/gh`, `~/.git-credentials`, `~/.netrc`, cargo's),
- reach only the package registries (crates.io, npm, PyPI, GitHub's downloads), and no Unix socket, so neither herdr's socket nor a tmux server,
- run kebbie's own task commands (`peers`, `tell`, `ask`, `report`, `steps`, `routine ok`, `link`, `backlog add`, `config`) outside the sandbox, matched on kebbie's absolute path, since those write kebbie's state. No user-only command is ever among them.

## The guard checks the caller's sandbox too

kebbie's own guard proves the caller's capability as well as its process tree. A process the kernel reports as sandboxed (macOS), or one that cannot create a file in kebbie's state directory under the account's home (both platforms), is refused every user-only command, whatever its chain of parents looks like.

When the chain is the lead's and the lead itself runs in a sandbox your own Claude Code settings turn on, kebbie refuses the same way and says so: add kebbie's path to `sandbox.excludedCommands` in `~/.claude/settings.json`.

A task-side command that fails because it ran inside the sandbox (spelled with `cd`, a pipe, a redirect or `$(…)`, so Claude Code did not take it out) says to run it on its own.

## The sandbox covers the shell only

Claude's file tools, hooks and MCP servers run outside it. That is why the file hook and deny rules above hold the file tools, and why the hooks kebbie installs run kebbie itself.

kebbie's own git runs unsandboxed in a sandboxed task's worktree, to read, switch and rebase it. There it runs with `core.hooksPath=/dev/null` and `core.fsmonitor=false`, so a hook or fsmonitor the worktree holds, as husky keeps its hooks in the repo, never runs as you through kebbie.

## A sandboxed task's hooks never run as you

A push you choose from the board runs the repo's hooks, except when they live in a sandboxed task's own worktree. Then kebbie runs none of them, and when one is a pre-push hook it does not push, says so, and leaves the push to you. Hooks in the main checkout's `.git/hooks` run as always, since neither the task's shell nor its file tools reach them.

## What the sandbox needs

- Claude Code 2.1.285 or later. With an older one kebbie starts no task, and `kebbie doctor` names the update.
- On Linux, bubblewrap and socat. Unix sockets are blocked only with the seccomp filter from `@anthropic-ai/sandbox-runtime`. When one is missing, kebbie refuses to start a task and names what to install.

`kebbie doctor` shows the sandbox's state on both platforms.

## Profiles can open the sandbox

A profile can name its own hosts (`allowed_domains`), folders (`allow_write`) and commands outside the sandbox (`excluded_commands`); see [Give tasks a profile](../how-to/give-tasks-a-profile.md).

The built-in `ops` profile runs `ssh`, `scp` and `rsync` outside it, since they ignore the sandbox's proxy and cannot connect from inside. An ssh session reaches other machines, and this one as you, so kebbie says so when an `ops` task starts.

With `task.sandbox = false` a task's shell runs unsandboxed, and kebbie's guards are what remain. `kebbie doctor`, a task's start and the board's Areas page say so.

## Where it ends

kebbie's guards read the process tree and files in your own account. A process running as you outside the sandbox can get past them. For example, it can:

- rewrite the worktree's push URL back, set another one, or forge the pre-push hook's token,
- write files through a script, past the vault's Problem gate,
- type into your shell through herdr.

## Outside the sandbox only the guards hold

The sandbox closes those to a task's shell, but not to a command that runs outside it: an `ops` task's ssh, a profile's own `excluded_commands`, or any task with `task.sandbox = false`. With the sandbox off, only kebbie's guards hold a task, and they read the process tree, which a script the task starts can leave. On your machine that is your choice to make.

## Package caches are code you run later

The package caches the shell may write are code your own builds load later: a crate's source under `~/.cargo/registry` or a package under `~/.npm` changed by a task runs when you next build unsandboxed. A folder a profile adds with `allow_write` is the same.

> **Warning:** Work on code you do not trust needs OS-level separation: another user account, a container or a VM.

## Settings come from your home only

kebbie reads its configuration only from files in your home directory, never from a file inside a repo. Cloning a repo cannot change how kebbie behaves, which commands it runs or who may push.

Its state is private: the state directory is `0700`, its files `0600`, and each file is replaced whole on write.

## Some repos get no push block

The push block uses git's per-worktree config. A repo whose shared git config sets `core.worktree` or `core.bare = true` gets no block until those are moved; `kebbie start` and `kebbie doctor` say so.
