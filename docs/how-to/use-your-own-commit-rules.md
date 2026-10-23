# Use your own commit rules

Make tasks commit the way your team does. kelpie leaves commit style to a skill: the task follows it, the reviewer checks a ready task against it, and `tidy` reshapes commits by it.

1. Write a Claude Code skill with your rules: a plugin's skill, or one in `~/.claude/skills` or the repo's `.claude/skills`. kelpie's default, [`kelpie:work-rules`](../../skills/work-rules/SKILL.md), is a starting point.

2. Name it in `~/.config/kelpie/config.toml` for every repo:

   ```toml
   [task]
   rules_skill = "my-rules"
   ```

   Or in a repo config for one repo, `~/.config/kelpie/repos/app.toml`.

New tasks follow it from their start, and a running task once it restarts. The reviewer checks by it from the next ready report, and when the reviewer gives no verdict, kelpie asks the lead to check by the same skill. kelpie's own check of each commit's form (a subject, a blank line and a body) runs only with `kelpie:work-rules`; with your skill, the reviewer checks the form your rules ask for.

## Reshape a task's commits

Press `t` on the board, or:

```bash
kelpie tidy billing "squash into one"
```

The task's Claude reshapes its commits by the rules skill, keeping the final tree identical.
