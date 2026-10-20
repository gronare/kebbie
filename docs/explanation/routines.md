# Routines

A routine is a check kelpie runs on a schedule: the health of a cluster every six hours, the dependency updates of an app every Monday morning. Each run is an ordinary kelpie task. It gets a worktree, a Space and a Claude, it investigates what the routine asks, and it ends in one of three ways: all is well, it reports what it found, or it fixes it.

## What a run is

You write a routine as a `[[routines]]` table, in config.toml or in a repo's own config (see [`[[routines]]`](../reference/configuration.md#routines)). It names the repo the run works in, a schedule, a prompt in your words and a mode.

When a run is due, kelpie starts a task named `<id>-<yyyymmdd-hhmm>` in that repo through the same start path as `kelpie start`, without taking focus. The task's first prompt is built from the routine: its title, your prompt, the mode's instructions and how to finish. The run's Claude works in Claude's `auto` permission mode, since nobody is there to answer a permission prompt, and without a vault note: a run that finds nothing leaves nothing behind.

On the board a run's task shows a `routine` marker next to its name and reports like any other task. The lead knows routine tasks come from kelpie and treats them like the rest.

## When it runs

The keeper checks the routines every minute. A routine is due:

- with `every`, once that interval has passed since its last run started; a new routine runs at the first check,
- with `at`, once that local time has come since its last run started; a new routine waits for the next time.

A machine that slept through several slots runs the routine once when it wakes, not once per missed slot. A run never starts while the last run's task is still open: the next one waits until you, the lead or the task itself closed it. With `[keeper] enabled = false` an open board runs the checks instead, and a lock keeps two boards from starting the same run.

`kelpie routines` shows each routine's next run and the outcome of its last one. `kelpie routine run <id>` starts a run at once, by the same rules.

## The modes

| Mode | The task | When all is well | Otherwise |
| --- | --- | --- | --- |
| `report` | investigates and changes nothing | `kelpie routine ok "<summary>"` | lists its findings on a sign-off card and waits under Needs you |
| `plan` | investigates and changes nothing until you approve | `kelpie routine ok "<summary>"` | writes a plan and asks you to approve it with `kelpie ask --plan`, then carries it out in commits |
| `fix` | investigates and fixes | `kelpie routine ok "<summary>"` | fixes what it found in commits and reports ready, with a card of what it found and changed |

`kelpie routine ok` records the run as ok and closes the task, which works only while the task has no changes and no commits. Only that task's own Claude can say it.

## Outcomes and notifications

Each routine keeps its last run in the state directory: when it started, its task, its outcome and a one-line summary. The outcome is `running` from the start, `ok` after `kelpie routine ok`, `findings` from the task's first report or question, and `failed` when the run could not start or its task closed before it said anything.

A run with findings sends `notify.command` a `routine-findings` event in place of that report's usual event, and a failed run sends `routine-failed`. An ok run sends nothing, so a quiet night stays quiet. See [Notify command](../reference/configuration.md#notify-command).

## The push exception

A task never pushes, and the lead pushes a task only under a permission you gave. A fix routine with `push = true` is that permission given in advance: when the run starts, kelpie gives its task the same push permission as the board's `a` key. Once the task reports ready and the lead finds its commits hold to your rules, the lead lands and pushes the fix without waiting for you, as it would any task you let it push. Use it for fixes you would accept unseen, such as an incident runbook step or a routine bump. Without `push = true` you push a routine's fix yourself.

## Related

- [Add a routine](../how-to/add-a-routine.md)
- [Let the lead push](../how-to/let-the-lead-push.md)
- [The trust boundary](trust-boundary.md)
