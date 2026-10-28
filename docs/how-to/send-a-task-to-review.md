# Send a task to review

Hold a risky task's push until an independent, read-only Claude has passed its code.

1. Select the task on the board and press `v`, or run:

   ```bash
   kebbie review billing
   ```

   The task moves to In review. Its push waits.

2. Wait for the verdict. The row shows `reviewed` on a pass or `review: N findings`.

   On findings, the lead passes the ones that matter to the task, which fixes them and reports ready. kebbie then reviews the changed code again.

3. Press `p` once the task shows under Ready to push. The reviewer's pass on the current code is its rules approval too.

## Pick the reviewer's model

```toml
[review]
model = "opus"
```

## Hold changes that leave the docs behind

In a repo with `docs.required`, the reviewer also checks the docs. When the commits change what a user sees or does and touch no README or `docs/` file, the verdict is `changes` with a finding such as:

```
- src/peers.rs:88 — documents nothing for the new `--json` option of `kebbie peers`; update docs/reference/commands.md (keep-docs).
```

See [Keep docs in step with changes](keep-docs-in-step.md).

## Read the verdict

The full verdict is `verdicts/<task>@<repo>.md` in the state directory. It holds `ok` or `changes`, the sha it reviewed, and findings with `file:line`, bugs first.

## Take it back out

Press `v` again, or run:

```bash
kebbie review --withdraw billing
```

The reviewer stops, its verdict goes, and push is offered again. Only you can withdraw.

See also: [The review gate](../explanation/review-gate.md).
