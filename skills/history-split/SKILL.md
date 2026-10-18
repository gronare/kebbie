---
name: history-split
description: Open tig, a terminal git history browser, in a herdr split next to this pane.
disable-model-invocation: true
---

# History split

1. **Check.** `HERDR_ENV` is `1` and `tig` is on `PATH`. When one fails, say which (the split opens only inside herdr, and it runs tig) and stop. Done when both hold.
2. **Open.** From the repo, run `sh "${CLAUDE_SKILL_DIR}/../../scripts/review.sh" history`. Done when it prints the new pane's id.
3. **Reply** in one line: tig lists the branch's commits; Enter opens a commit's diff, and `q` steps back, closing the split from the list.
