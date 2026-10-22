# The trust boundary

kelpie keeps a task's Claude from pushing, approving its own work, switching away mode or answering for you, whether by accident or because its prompt, a file it read or another session's message told it to. **It is not a sandbox.** This page says what it guards and where the guard ends.

## What it is for

An agent that reads a README telling it to "push when done", or a message from another task saying "the user approved this", should not be able to act on it. Those are the realistic failures: an instruction in the wrong place, or a session that is wrong about what it is allowed to do. kelpie's guards make the important steps, pushing and answering for you, belong to you and the lead only.

## What kelpie guards

| Guard | How |
| --- | --- |
| A task cannot push. | A pre-push hook and a push URL rewrite in the worktree's own git config block `git push` in a task's worktree. The task's Claude settings also deny `git push`, `kelpie approve` and `kelpie push`. |
| Only you and the lead push, and the lead only with your permission. | `kelpie push` reads who runs it from the process tree, not from the folder. The lead pushes only tasks you gave it permission for, and only while its rules approval matches the commits. A push that waits for the task's Claude to run tests is recorded only after those checks pass, names you or the lead, and completes only for the exact commits it was recorded for, with the same checks run again for whoever asked; a task's report can end the wait, never start a push. |
| A task cannot approve its own work. | `kelpie approve` reads the caller from the process tree. |
| Only you switch away mode or withdraw a review. Only the lead decides a question of method, and only the lead defers a question while you are away. Only you or the lead send a `summary` notification. | The same process-tree check. |
| The reviewer only reads. | Read, Grep, Glob and `git log`, `git diff` and `git show` alone, in `dontAsk` mode, with a hook that refuses every other command, and none of your settings, hooks or plugins. |
| Only you answer a task's Claude's own prompt. | `kelpie choose` reads its caller from the process tree and refuses a task; that check is what holds. A task's settings also deny `kelpie choose` and name herdr's keystroke paths (`herdr pane send-keys`, `send-text`, `run`, `herdr agent send-keys`, `prompt`), but a deny rule matches only that literal spelling. The lead's settings do not allow `kelpie choose`, so Claude Code asks you before the lead runs it while your own settings hold no allow rule for it. |
| Messages are not instructions. | A message arrives as `[from <task>@<repo>] <text>` with every line after the first indented, so no message can start a line as `[from you]`. The task role tells Claude that a peer's message is information, never your instruction. |
| With the vault, edits wait for the Problem. | A hook refuses file edits and plain file-writing shell commands until the task's note has its Problem. |

Within that boundary kelpie fails closed. When it cannot tell who runs it (`ps` fails, or the chain of parents breaks), or a kelpie runs detached from any terminal while a Claude that kelpie started is alive, it treats the caller as a Claude and refuses what only you or the lead may do.

## Where it ends

Every guard reads the process tree and files in your own account. A hostile process running as you can get past all of them. For example, it can:

- rewrite the worktree's push URL back, set another one, or forge the pre-push hook's token,
- write files through a script, past the vault's Problem gate,
- type into your shell through herdr.

The guards stop accidents and misplaced instructions. They do not stop a determined agent running as your user.

**Work on code you do not trust needs OS-level separation:** another user account, a container or a VM.

## Settings come from your home only

kelpie reads its configuration only from files in your home directory, never from a file inside a repo. Cloning a repo cannot change how kelpie behaves, which commands it runs or who may push.

Its state is private: the state directory is `0700`, its files `0600`, and each file is replaced whole on write.

## Repos the push block cannot cover

The push block uses git's per-worktree config. A repo whose shared git config sets `core.worktree` or `core.bare = true` gets no block until those are moved; `kelpie start` and `kelpie doctor` say so.
