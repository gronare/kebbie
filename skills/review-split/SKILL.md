---
name: review-split
description: Open a herdr split to review a piece of work before it is committed or pushed — the work's files in review order with diff previews, unified diffs in nvim, comments sent back to this Claude pane, read marks that hold until a file changes. Use only when the user runs /review-split or asks for a review split.
argument-hint: "[base or what to review]"
disable-model-invocation: true
---

# Review split

1. Stop if `HERDR_ENV` is not `1`: say the review split only works inside herdr. Stop too if any of `fzf`, `nvim`, `jq` is missing from `PATH`, and name the missing ones.
2. Work out what is under review. `$ARGUMENTS` may name a base (`origin/main`, `HEAD~3`) or describe the work.
   - Work you did in this session: the files you changed.
   - Anything else: establish the work before opening the split. Read the unpushed commits and their messages (`git log @{upstream}..`), the uncommitted changes, and whatever task note, plan or log the project keeps for this work. When the scope is still unclear, ask the user instead of guessing.
   - Base: the commit before the work's first commit when part of it is committed, `HEAD` when it is all uncommitted, or the base the user gave.
   - When the user wants every change rather than one piece of work, skip the scope and pass no paths.
   - In a linked git worktree (`git rev-parse --git-dir` differs from `--git-common-dir`), everything since it left the default branch is the work: pass no base and no paths, and the script bases the review on that fork point.
3. From the repo root, run `sh "${CLAUDE_SKILL_DIR}/../../scripts/review.sh" split <base> <path>...` with the work's files as repo-relative paths, specs included, in the order you will present them. Changes outside the scope stay out of the list until the user presses ctrl-o. When a review pane is already open next to you, the same command rescopes it instead of opening a second one.
4. Reply with a short review guide, numbered in the same order as the paths, no more than a few lines per file: what changed and which choice is worth a look. Do not repeat the diff. Mention in one line which other changes you left out of scope, if any.
5. Add one line on the controls: Enter opens a file's diff, c there comments, o opens the real file over it, r marks it read; `?` in the list shows every key. Comments are not sent until the user presses Enter here.

When comments arrive as `path:line — text` lines (`(before)` means the line number is in the old version), answer each one; change code only where a comment asks for it. A `path:lines — edited by me during the review` line means the user changed that file by hand: read it again before touching it, and keep their edit unless they say otherwise.
