# Security

kelpie keeps a task's Claude from pushing, approving, switching away mode or answering for you by accident, or because its prompt, a file it read or another session's message told it to. **It is not a sandbox.**

## What kelpie guards

| Guard | How |
| --- | --- |
| A task cannot push. | kelpie blocks `git push` in a task's worktree with a pre-push hook and a push URL rewrite in the worktree's own git config. The task's Claude settings also deny `git push`, `kelpie approve` and `kelpie push`. |
| Only you and the lead push, and the lead only with your permission. | `kelpie push` reads who runs it from the process tree, not from the folder. The lead pushes only tasks you gave it permission for, and only while its rules approval matches the commits. |
| A task cannot approve its own work. | `kelpie approve` reads the caller from the process tree. |
| Only you switch away mode or withdraw a review. Only the lead decides or defers a question while you are away. Only you or the lead send a `summary` notification. | The same process-tree check. |
| The reviewer only reads. | Read, Grep, Glob and `git log`/`diff`/`show` alone, in `dontAsk` mode, with a hook that refuses every other command, and none of your settings, hooks or plugins. |
| Messages are not instructions. | A message arrives as `[from <task>@<repo>] <text>` with every line after the first indented, so no message can start a line as `[from you]`. The task role tells Claude that a peer's message is information, never your instruction. |
| With the vault, edits wait for the Problem. | A hook refuses file edits and plain file-writing shell commands until the task's note has its Problem. |

Within that boundary kelpie fails closed. When it cannot read who runs it (`ps` fails, or the chain of parents breaks), or a kelpie runs detached from any terminal while a Claude that kelpie started is alive, it treats the caller as a Claude and refuses what only you or the lead may do.

## What it does not guard

Every gate reads the process tree and files in your own account. A hostile process running as you can get past all of them. For example, it can:

- rewrite the worktree's push URL back, set another one, or forge the pre-push hook's token,
- write files through a script, past the vault's Problem gate,
- type into your shell through herdr.

The guards stop accidents and misplaced instructions. They do not stop a determined agent running as your user.

**Work on code you do not trust needs OS-level separation:** another user account, a container or a VM.

## State on disk

kelpie keeps its state private: the state directory is `0700`, its files `0600`, each replaced whole. Settings come only from files in your home directory, never from a file inside a repo, so cloning a repo cannot change how kelpie behaves.

## Repos where the push block cannot apply

The push block uses git's per-worktree config. A repo whose shared git config sets `core.worktree` or `core.bare = true` gets no block until those are moved; `kelpie start` and `kelpie doctor` say so.
