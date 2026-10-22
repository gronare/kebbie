# Send a task to review

Hold a risky task's push until an independent, read-only Claude has passed its code.

1. Select the task on the board and press `v`, or run:

   ```bash
   kelpie review billing
   ```

   The task moves to To review. Its push waits.

2. Wait for the verdict. The row shows `reviewed` on a pass or `review: N findings`. The lead passes the findings that matter to the task, which fixes them and reports ready. kelpie then reviews the changed code again.

3. Once the reviewer has passed the current code, the task shows under Ready to push: the pass is its rules approval too. Press `p`.

## Pick the reviewer's model

```toml
[review]
model = "opus"
```

## Hold changes that leave the docs behind

In a repo with `docs.required`, the reviewer also checks the docs. When the commits touch no README or `docs/` file and change what a user sees or does, the verdict is `changes` with a finding such as:

```
- src/peers.rs:88 — documents nothing for the new `--json` option of `kelpie peers`; update docs/reference/commands.md (keep-docs).
```

See [Keep docs in step with changes](keep-docs-in-step.md).

## Read the verdict

The full verdict is `verdicts/<task>@<repo>.md` in the state directory: `ok` or `changes`, the sha it reviewed, and findings with `file:line`, bugs first.

## Take it back out

Press `v` again, or:

```bash
kelpie review --withdraw billing
```

The reviewer stops, its verdict goes, and push is offered again. Only you can withdraw.

See also: [The review gate](../explanation/review-gate.md).
