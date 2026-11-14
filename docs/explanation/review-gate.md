# The review gate

This page explains the rules check every ready task gets, the assessment of whether its work needs a review, the reviewer gate, and what keeps a pass.

kebbie has two kinds of review, and they answer different questions. The review split is you reading the change. The reviewer gate is a second Claude reading it, with a verdict that holds the push until it passes.

## Every ready task gets a rules check

The rules check is about how the work is shaped.

1. kebbie checks each commit's form itself: with `kebbie:work-rules`, a ready report is refused while a commit lacks a subject, a blank line and a body.
2. The keeper (or the board while the keeper is off) starts the reviewer from its own environment, never the task's, with a brief of the rules alone. It reads each commit against your rules skill, and in a repo with `docs.required` against the docs too.
3. A pass clears the task's rules check, and the lead hears nothing. A break goes to the task with the commit and the rule, and to the lead as one line.

## The lead checks when the reviewer cannot

When the reviewer gives no verdict (it fails, or has no answer within 15 minutes), the lead is asked to check the rules instead.

Each code is checked once. A ready report on code the reviewer found breaks in gets those findings back, and one on code it gave no verdict on goes to the lead. Commits that change while the check runs get one more run.

## The reviewer gate holds the push until the code is right

A task in review, sent by you (`v` on the board, or `kebbie review`), by the lead, or by kebbie under [`review.auto`](#an-assessor-judges-whether-the-work-needs-a-review), waits under In review until a separate Claude has read it and given `verdict: ok` on the code its HEAD makes. Until then the board offers no push for it.

A full review reads the rules too, so its pass clears the rules check too, and no separate rules check runs while a task is in review. A task in review shows under Ready to push once the reviewer has passed its current code.

## A separate Claude reads it like a colleague

The task's own Claude has written the change and believes in it. The lead has the whole picture but also a stake in moving work along. A reviewer that starts fresh, with no part in writing the code, reads it the way a colleague would.

So the reviewer gets what a colleague would want and nothing more:

- the [`kebbie:review`](../../skills/review/SKILL.md) skill as its instructions,
- the commits against the default branch,
- the task's prompt and later messages, so it knows what was asked,
- your rules skill,
- the repo's `task.also_noticed`, which draws the task's scope: under `"backlog"` and `"ask"` a change the task did not need is a finding, and under `"fix"` a small fix in a commit of its own is not.

It runs with none of your own settings, hooks or plugins.

## The reviewer checks the docs too

In a repo with `docs.required`, the reviewer also reads the commits for what a user sees or does, and fails a branch that changes it without a word in the README or `docs/`.

kebbie hands it the docs the commits change, so it judges whether they describe the change rather than whether some doc moved. The reviewer, not a list of paths, judges what counts as user-visible, since a flag, a notice text or a config key can live in any file.

## The reviewer only reads

A reviewer that can edit will fix what it finds, and then nobody has reviewed the fix. A Claude reviewer gets Read, Grep, Glob and the shell for `git log`, `git diff` and `git show` alone, in `dontAsk` mode, with a hook that refuses every other command. Each git read runs with `--no-ext-diff --no-textconv` and `core.fsmonitor` off, so the repo's config cannot change the diff it reads.

A review profile that runs Codex runs `codex exec` in Codex's read-only sandbox: its commands can read but neither write nor reach the network, and it loads none of your Codex config, its MCP servers, plugins and rules included, and keeps no session file. The keeper starts it as it starts a Claude reviewer, from its own environment and with the `codex` on its own PATH.

Its findings go to the lead, which passes the ones that matter back to the task. The task fixes them and reports ready again, and kebbie starts the reviewer on the changed code.

## You choose the reviewer, and a second opinion holds nothing

The reviewer is a profile: `review.profile` names it, the built-in `reviewer` unless you pick a review profile of your own. A review profile takes only what a reader needs, its agent, model, effort and role, so no profile can hand a reviewer write access or permission rules. Its verdict is the one the push waits on.

A second reviewer, on another model or another agent, often catches what the first misses. But two verdicts that both hold the push would make every task wait on the slower one, and a disagreement between them would have no owner. So `review.second` names a second opinion that never holds the push: its points go to the lead, which weighs them and passes on the ones that matter, as it does with a review's findings. You ask for it per task with `s` or `o` in the `v` menu, or have it run with every review.

The assessor, the rules check and the comment reader keep running the built-in `reviewer`, whichever profile `review.profile` names.

## A pass belongs to the code

A pass belongs to the code, not to the commits. A clean rebase, a reworded message or a squash keep the pass, because the code is the same. A changed line does not: the row shows `review stale`, and the reviewer runs again when the task next reports ready.

So you can tidy history in the push window without paying for a second review.

Findings belong to the commits instead, since a reworded message can be what they asked for. Any change to the commits, a reword included, makes them stale, and the reviewer reads the branch again when the task next reports ready, also while its CI runs.

## Test runs are recorded, not repeated

A task's Claude runs the tests related to its change before it reports ready; the full suite is CI's job after the push. A repo whose suite runs in minutes can ask for the full suite on HEAD before every push instead, with [`tests.before_push`](../reference/configuration.md#ci-updates-and-the-keeper). kebbie records each of those runs from the task's hooks, with its command, its result and the commit it ran on, so nobody has to run them again to know they passed.

The reviewer gets every run on the code under review in its brief, one line per command with how many runs passed and failed and the newest result. Only when none ran on that code does it get the newest older ones instead. It treats a failed run, or no passing run on the reviewed code, as a finding. A run whose result kebbie could not see counts as no run.

The brief also tells a missing record from a missing run. When the task's hooks run a kebbie that is no longer installed, or the hook that records runs never ran in the task, the reviewer is told the record is missing, so it does not take that as proof that no test ran. A command that looks like a test run but is not in [`task.test_commands`](../reference/configuration.md#keys) is named too.

A review that asked for tests does not wait for a code change. When the task records a passing run on the reviewed code after the verdict, it goes back to the reviewer by itself, and the lead hears the new verdict.

## The row shows the latest run

The board's task row and the push window show the latest run:

- green when it passed on the code the branch holds now,
- yellow once a commit changed the code after it,
- red when it failed,
- peach with `?` when kebbie saw the run but not its result.

Like a pass, a run belongs to the code, so a clean rebase or a squash keeps it current. See [`kebbie evidence`](../reference/commands.md#kebbie-evidence-name---json).

## An assessor judges whether the work needs a review

A full review costs a Claude run and some waiting, so not every task gets one. Whether one pays is a judgement of the work, not of its file names, so on a ready report a short, read-only assessor makes it. It is a fresh Claude, started by the keeper from its own environment, that never wrote the code.

It reads the task's whole work since it started, the subtasks already pushed included, with what kebbie measured: the files and lines changed, the test files, migration and schema files, and the number of commits. Its instructions, `roles/review-assess.md`, name what makes work worth a second reading: its size, its complexity, many tests changed, the database, business logic such as money or permissions, security, and agent instructions.

Its answer is one line on the task's row, in your terms: `review suggested: changes how VAT is rounded and its tests` or `no review: wording in two templates`. An assessor that gives no answer leaves `review not assessed`, which counts as a suggestion, never as no review.

## You choose what a suggestion does

[`review.auto`](../reference/configuration.md#review-and-docs) decides what kebbie does with work the assessor says needs a review:

- `"suggest"`, the default, starts nothing: the row and the lead say so, and you or the lead send it with `v`.
- `"run"` starts the reviewer by itself and lets the push go meanwhile.
- `"require"` holds the push from the ready report on, through the assessment and the review, until the reviewer passes.

## A finding only you can settle

Sometimes the code is right if you meant it, and the reviewer cannot tell from the conversation whether you did. The reviewer marks such a finding `confirm:`, a hunk outside the task `scope:`, and a test or docs gap the task cannot close `gap:`. kebbie's investigator reads each first; with it off (`investigate.profile = "none"`), or when it cannot decide, a verdict whose every finding is marked comes to you under Needs you:

- Confirm passes the review on the code it read, with your words kept in the verdict. The task hears them and changes nothing.
- Change it sends your comment to the task, which reworks it and goes back to the reviewer.

`kebbie review --confirm <task> "<your words>"` does the same as Confirm from your terminal. The lead cannot answer it for you: it has the finding [investigated](obstacles-and-findings.md#review-findings-are-resolved-not-passed-along) and brings you what it found, such as your own words on it in the task's note, beside the question.

## Only you take a task out of review

You can take a task back out with `v` and then `v`, Withdraw from review, or `kebbie review --withdraw`. Only you can withdraw: neither the lead nor a task can talk its way past the gate. The assessor leaves a task you withdrew out until you send it again.

The gate also carries push permission. On the Areas page, `a` lets the lead push every task in a repo once its reviewer has passed the current code. See [Let the lead push](../how-to/let-the-lead-push.md).
