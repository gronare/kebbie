# Send a task to review

Hold a task's push until an independent, read-only reviewer has passed its code, and ask a second reviewer for its opinion when you want one.

On a ready report, an assessor judges whether the work needs a review, and the row says `review suggested: <why> · v` when it does. See [An assessor judges whether the work needs a review](../explanation/review-gate.md#an-assessor-judges-whether-the-work-needs-a-review).

1. Select the task on the board and press `v`. A menu opens:

   ```
   ╭ review · billing-vat ──────────────────────────────────────────────╮
   │ 3 commits since main · assessor: review suggested, touches money   │
   │                                                                    │
   │ ▸ Review                                                         v │
   │   reviewer · claude · opus · max                                   │
   │ ↑ ↓ choose · ⏎ run · esc close                                     │
   ╰────────────────────────────────────────────────────────────────────╯
   ```

   Press `v` again, or run:

   ```bash
   kebbie review billing
   ```

   The task moves to In review. Its push waits.

2. Wait for the verdict. The row shows `reviewed` on a pass or `review: N findings`.

   On findings, the lead passes the ones that matter to the task, which fixes them and reports ready. kebbie then reviews the changed code again.

3. Press `p` once the task shows under Ready to push. The reviewer's pass on the current code clears its rules check too.

## Choose who reviews

The reviewer is the review profile `review.profile` names, the built-in `reviewer` unless you pick another. Its verdict is the one the push waits on.

1. Add a review profile: on the Settings page (`,`), go to Profiles, press `n`, and answer Review to "What is it for?". See [Add a review profile](give-tasks-a-profile.md#add-a-review-profile).
2. On the Settings page, select `review.profile` under Review & docs and press `⏎`. The pick lists review profiles only.

Or set it by hand:

```toml
[review]
profile = "careful-review"
```

## Pick the reviewer's model

`review.model` sets the model of the reviewer `review.profile` names, ahead of the model in its profile:

```toml
[review]
model = "opus"
```

`m` on that profile's row on the Settings page writes it.

## Ask for a second opinion

A second opinion is another review profile reading the same commits, for example one that runs Codex. It never holds the push: its points go to the lead, which passes them to the task as it does a review's findings.

1. On the Settings page, select `review.second` and press `⏎`. Pick a review profile, or Always to run it with every review.

   ```toml
   [review]
   second = "codex-review"
   ```

   `"always codex-review"` runs it with every review.

2. Press `v` on a task. The menu now offers:

   - `s` Review with a second opinion: the reviewer, and the second opinion beside it.
   - `o` Second opinion only: the push still waits on the reviewer when the task is in review.

   Or run `kebbie review --second billing`, or `kebbie review --second-only billing`.

3. Read the points. The row shows `second opinion: codex-review reading…`, then `second opinion: 2 points · v i reads them`. Press `v`, then `i`, to page through them. The lead passes the ones that matter to the task.

The points are `second-opinions/<task>@<repo>.md` in the state directory, in the same shape as a verdict.

## Hold changes that leave the docs behind

In a repo with `docs.required`, the reviewer also checks the docs. When the commits change what a user sees or does and touch no README or `docs/` file, the verdict is `changes` with a finding such as:

```
- src/peers.rs:88 — documents nothing for the new `--json` option of `kebbie peers`; update docs/reference/commands.md (keep-docs).
```

See [Keep docs in step with changes](keep-docs-in-step.md).

## Read the verdict

The full verdict is `verdicts/<task>@<repo>.md` in the state directory. It holds `ok` or `changes`, the sha it reviewed, and findings with `file:line`, bugs first.

## Take it back out

Press `v`. On a task in review the menu's first answer is Withdraw from review, so `v` again takes it out. Or run:

```bash
kebbie review --withdraw billing
```

The reviewer stops, its verdict goes, a second opinion with it, and push is offered again. Only you can withdraw, and the assessor leaves a task you withdrew out until you send it again.

## Let kebbie start or require the review

To have the reviewer start by itself on work the assessor says needs one, set in `~/.config/kebbie/config.toml` or a repo's own file:

```toml
[review]
auto = "run"
```

`"require"` also holds the push until the reviewer passes. `"suggest"`, the default, leaves the sending to you.

See also: [The review gate](../explanation/review-gate.md).
