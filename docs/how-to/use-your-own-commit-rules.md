# Use your own commit rules

Make tasks commit the way your team does. kebbie leaves commit style to a skill: the task follows it, the reviewer checks a ready task against it, and `tidy` reshapes commits by it.

1. Write a Claude Code skill with your rules: a plugin's skill, or one in `~/.claude/skills` or the repo's `.claude/skills`. kebbie's default, [`kebbie:work-rules`](../../skills/work-rules/SKILL.md), is a starting point.

2. Name it in `~/.config/kebbie/config.toml` for every repo, or in a repo config such as `~/.config/kebbie/repos/app.toml` for one repo:

   ```toml
   [task]
   rules_skill = "my-rules"
   ```

3. Start a task, or restart a running one. New tasks follow the skill from their start, and a running task once it restarts. The reviewer checks by it from the next ready report.

- If the reviewer gives no verdict, kebbie asks the lead to check by the same skill.
- If you use your own skill, the reviewer checks the commit form your rules ask for. kebbie's own check of each commit's form (a subject, a blank line and a body) runs only with `kebbie:work-rules`.

## Reshape a task's commits

1. Press `t` on the board, or run:

   ```bash
   kebbie tidy billing "squash into one"
   ```

   The task's Claude reshapes its commits by the rules skill, keeping the final tree identical.
