---
name: work-rules
description: kebbie's default commit rules for a task's branch: a subject and a body that says why, one logical change per commit, staging by explicit file list, the push left to the user. Use when kebbie's task role names it, when you commit or reshape commits, and when a ready task's commits are checked.
---

# Work rules

The branch is the task's **history**: what a reviewer reads, one commit at a time, to learn why the code is the way it is. Every commit on it holds to all four rules below; the branch is ready when each of its commits does.

1. **Subject, blank line, body.** The subject names what changed. The body says why the change was needed and what the code now does, so a reader who sees only this commit understands it.
2. **One logical change per commit.** A reviewer reads one idea at a time and a revert takes one idea back. A refactor the change needs gets its own commit beside it. A commit that tweaks, fixes or undoes an earlier commit on the branch is folded into that commit, so the branch carries finished changes, never a trail of attempts.
3. **Explicit file lists.** Stage with `git add <file> <file>…`, naming each file the commit carries, so it holds only this change even when other sessions share the repo.
4. **The user pushes.** The task ends at a branch reported ready; the push goes through kebbie's Push on the user's word.

Example:

```
Count free worktrees per repo on the board

The header summed free worktrees across every repo, though a pool
belongs to one repo, so "1 free" said nothing about the repo you
were about to start in. It now names each repo with free ones.
```

## Checking a branch

A ready task's branch is checked with `git -C <worktree> log -p <default branch>..HEAD`: by the reviewer kebbie runs when the task reports ready, or by the lead when the reviewer gives no verdict. Judge every commit on rules 1 and 2, and on rule 3 by any file it carries that belongs to another change; the check is done when each commit has a verdict on each. Each break names the commit, the rule by its number and what to fix. The lead sends a break back with `kebbie tell <task> "<commit>: <rule>, <what to fix>"`, and when every commit holds, runs `kebbie -C <repo> approve <task>`.
