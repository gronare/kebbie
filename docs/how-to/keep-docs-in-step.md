# Keep docs in step with changes

Hold every change to what users see or do until its README and `docs/` pages say so too. For a public repo, where the docs are how people learn the product, this keeps main and its documentation telling the same story.

1. Turn it on in the repo's config, `~/.config/kebbie/repos/app.toml`:

   ```toml
   [docs]
   required = true
   ```

   It is a repo config key only: each repo opts in on its own. `kebbie doctor`, run inside a repo that is public on GitHub, suggests it while it is off.

2. Check the value in effect:

   ```bash
   kebbie config --repo ~/code/app
   ```

New tasks in the repo follow it from their start.

## What a task does

A task's Claude runs the [`kebbie:keep-docs`](../../skills/keep-docs/SKILL.md) skill when its change alters what a user sees or does. The skill says what counts, and which page of the README or `docs/` each change belongs on, by the [Diátaxis](https://diataxis.fr) quadrants this documentation follows. A task that runs Codex is told the same in its role, and finds the page from the index in `docs/README.md`, or the README when there is none.

## What the reviewer does

Both reviewers check the docs: the read-only rules check that starts when the task reports ready, and the full review when you [send the task to review](send-a-task-to-review.md). Each reads the commits for what a user sees or does, by the same skill, and gets the list of docs the commits change: the root README, anything under `docs/`, and a workspace package's own README. Each change those docs leave undescribed is a finding:

```
verdict: changes

- src/peers.rs:88 — documents nothing for the new `--json` option of `kebbie peers`; update docs/reference/commands.md (keep-docs).
```

The task updates the page, reports ready, and the reviewer reads the branch again. A refactor, a test or an internal rename passes without docs.

See also: [`docs.required`](../reference/configuration.md#keys), [The review gate](../explanation/review-gate.md).
