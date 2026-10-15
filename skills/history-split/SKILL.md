---
name: history-split
description: Open tig (a terminal git history browser) in a herdr split next to this pane. Use only when the user runs /history-split or asks for a history split.
disable-model-invocation: true
---

# History split

1. Stop if `HERDR_ENV` is not `1`: say the history split only works inside herdr. Stop too if `tig` is missing from `PATH`.
2. From the repo, run `sh "${CLAUDE_SKILL_DIR}/../../scripts/review.sh" history`.
3. Reply in one line: tig lists the branch's commits; Enter opens a commit's diff, `q` steps back and closes the split from the list.
