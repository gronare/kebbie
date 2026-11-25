---
name: work-rules
description: kebbie's default commit rules for a task's branch: the work cut into small increments, each commit green and safe to deploy on its own, preparatory ones first and risky ones last, a subject and a body that says why, one logical change per commit, staging by explicit file list, the push left to the user. Use when kebbie's task role names it, when you plan, commit or reshape commits, and when a ready task's commits are checked.
---

# Work rules

The branch is the task's **history**: what a reviewer reads, one commit at a time, to learn why the code is the way it is, and what goes out, trunk-based, one push at a time. Each commit is an **increment**: one deliverable, reviewable alone and safe to deploy alone, so once it is pushed it is out of sight and out of mind.

- **Make the change easy, then make the easy change.** Preparatory commits that change no behaviour (a move, a rename, an interface, an expand) come first; the behaviour change, now small, after them; a risky or irreversible step last and alone. Plan the cut before your first line of code, and check your branch before you report ready, by [CUT.md](CUT.md).
- **The branch's order is the push order.** A branch is pushed from its start, the accepted run of commits from the top, so each commit stands on the ones before it. A preparatory commit made late moves up with `kebbie order move <sha> --before <sha> --because "<why>"`, and commits that only work together go out as one with `kebbie order group <sha> <sha> --because "<why>"`.
- **With subtasks on** (`task.steps`; your role then has a Subtasks section), the increments are grouped into titled **subtasks** in the order they go out, each a slice of the work: one sub-problem, or one part of it in its order. Each title is in the vault's language and names the change the user gets, never a file or a word from the code. Commits go to the first subtask not done, or to the one `kebbie subtasks on <id>` names; when its slice is finished, `kebbie subtasks done <id>` makes it ready for review, which kebbie refuses for a subtask without a commit or with a commit that breaks rule 1. The plan is living: `kebbie subtasks add "<subtask>" --because "<why>"` adds one the work turns out to need, and `kebbie subtasks "<subtask>" …` lists them anew when the split is wrong. With subtasks off, the commits alone carry the cut, under the same rules.

Every commit on the branch holds to all the rules below; the branch is ready when each of its commits does.

1. **Subject, blank line, body.** The subject names what changed. The body says why the change was needed and what the code now does, so a reader who sees only this commit understands it.
2. **One logical change per commit.** A reviewer reads one idea at a time and a revert takes one idea back. A refactor the change needs gets its own commit before it. A commit that tweaks, fixes or undoes an earlier commit on the branch is folded into that commit, so the branch carries finished changes, never a trail of attempts. A change to a commit already pushed is a new commit after the others: the history is the honest road to the goal.
3. **Explicit file lists.** Stage with `git add <file> <file>…`, naming each file the commit carries, so it holds only this change even when other sessions share the repo.
4. **The user pushes.** The task ends at a branch reported ready; the push goes through kebbie's Push on the user's word.
5. **Green alone.** Each commit builds and passes its tests checked out on its own, needing nothing a later commit adds. Build and test in **sweeps**: write the part whole before its first build, let one run list every error, failing test and lint warning (the linter in the same run as the compiler), fix them all, then run once more to verify. Each part's last commit gets that verifying run, so kebbie records a green run at every sha a push can end on.
6. **Safe to deploy alone.** Nothing breaks if the next commit never comes: behaviour that is not whole yet stays dark (unreached, or behind a flag that is off), and a changed shape goes expand, migrate, contract, each its own commit.
7. **Easy first, risky last.** Preparatory commits with no behaviour change come before the behaviour change they prepare. A data migration, an identity merge, a deletion, or the contract that drops an old shape comes last, in a commit of its own.
8. **Written for its own commit.** Names, comments, tests and the message fit this commit's deliverable, so a reviewer understands it from this commit alone. Nothing in it is explained only by a later commit: each test observes what this commit delivers, through what this commit exposes.

Example:

```
Count free worktrees per repo on the board

The header summed free worktrees across every repo, though a pool
belongs to one repo, so "1 free" said nothing about the repo you
were about to start in. It now names each repo with free ones.
```

## Checking a branch

A ready task's branch is checked with `git -C <worktree> log --reverse -p <default branch>..HEAD`, oldest commit first: by the task itself before it reports ready, by the reviewer kebbie runs when the task reports ready, or by the lead when the reviewer gives no verdict. Judge every commit on rules 1, 2 and 5 to 8 by [CUT.md](CUT.md)'s branch check, and on rule 3 by any file it carries that belongs to another change; the check is done when each commit has a verdict on each. Each break names the commit, the rule by its number and what to fix. The lead sends a break back with `kebbie tell <task> "<commit>: <rule>, <what to fix>"`, and when every commit holds, runs `kebbie -C <repo> approve <task>`.
