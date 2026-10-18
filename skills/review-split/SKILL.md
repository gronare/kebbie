---
name: review-split
description: Open a herdr split to review a piece of work before it is committed or pushed: the work's files in review order with diff previews, unified diffs in nvim, comments sent back to this Claude pane, read marks that hold until a file changes.
argument-hint: "[base or what to review]"
disable-model-invocation: true
---

# Review split

1. **Check.** `HERDR_ENV` is `1`, and `fzf`, `nvim` and `jq` are on `PATH`. When one fails, say so (the split opens only inside herdr), name the missing tools, and stop.
2. **Scope** the work under review: its base and its files. `$ARGUMENTS` may name a base (`origin/main`, `HEAD~3`) or describe the work. The first case that fits wins:
   - In a linked git worktree (`git rev-parse --git-dir` differs from `--git-common-dir`), everything since it left the default branch is the work: pass no base and no paths, and the script bases the review on that fork point.
   - The user wants every change rather than one piece of work: pass no paths.
   - Work you did in this session: the files you changed.
   - Anything else: establish the work from the unpushed commits and their messages (`git log @{upstream}..`), the uncommitted changes, and whatever task note, plan or log the project keeps for it. When the scope is still unclear, ask the user.

   The base is the one the user gave, else the commit before the work's first commit when part of it is committed, else `HEAD` when it is all uncommitted. Done when you have the base and the file list, or know to pass neither.
3. **Open.** From the repo root, run `sh "${CLAUDE_SKILL_DIR}/../../scripts/review.sh" split <base> <path>...` with the work's files as repo-relative paths, specs included, in the order you will present them. Changes outside the scope stay out of the list until the user presses ctrl-o. When a review pane is already open next to you, the same command rescopes it. Done when it prints the pane's id.
4. **Guide.** Reply with a short review guide, numbered in the order of the paths, a few lines per file at most: what changed and which choice is worth a look; the diff itself is in the split. Name in one line the changes you left out of scope, if any.
5. **Controls**, one line: Enter opens a file's diff, c there comments, o opens the real file over it, r marks it read; `?` in the list shows every key. Comments are not sent until the user presses Enter here.

## Comments

Comments arrive as `path:line — text` lines; `(before)` after the line number means it counts in the old version. Answer each one, and change code only where a comment asks for it.

A `path:lines — edited by me during the review` line means the user changed that file by hand: read it again before touching it, and keep their edit unless they say otherwise.
