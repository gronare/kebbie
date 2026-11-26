# The fault watcher

When you tell the lead something is wrong, the fault watcher makes it a kebbie issue with its root cause and queues the task that fixes the cause. Nothing waits on the lead remembering it, and kebbie counts whether the fault comes back once its fix is in use.

## What it reads

- **Your own words in a chat**: the lead's chat and each task's Claude conversation. Only what you typed counts: messages from tasks or kebbie, tool output and reports from other installs never do. A chat that began before the watcher first looked is read from where it stood then, never its history; one that began since, such as the lead's after a restart, is read from its first words. A compaction's summary and the prompts kebbie types into a chat, a task's start prompt and the lead's first prompt, are never your words.
- **Your answers on the board**: Ask for changes or Reject with a comment on a plan, Something is wrong on a check, and your own words in place of the options.

A short yes, a number or a pasted prompt is skipped. Everything else waits two minutes, so the agent's answer to it is in, and the watcher reads up to twelve cases at a time, at most every five minutes.

## What it decides

A read-only Claude, with the investigator's profile (the reviewer's when the investigator is off), weighs each case against kebbie's source at the build you run and kebbie's own issues on this machine. For each it answers one of:

| Verdict | What kebbie does |
| --- | --- |
| No fault in kebbie | Nothing. Your project's own bugs, a question, a thank-you. |
| A known issue | Records the occurrence on it. A closed issue opens again as come back. |
| A new fault | Files a kebbie issue with its root cause, the kind of barrier the fix needs (code, role or skill) and the files it touches, then queues its fix task. |
| Linked to a task | Files the issue for a fix a task started after your words already builds, so its return still counts. |
| Ask | Puts the one question about what you want under a backlog item's open questions, where the board asks it. |

## The fix task

kebbie queues one fix task per issue, named `fault-<id>`, in kebbie's repo, with the issue's text, its root cause and the barrier in its prompt; the lead gives it a recipe. At most three start in a day; the next waits until the oldest of the day's is a day old. A new occurrence while the task runs or waits is passed to it, and a task the watcher finds already fixing the fault becomes its fix task, so no fault gets two. Once a fix task has ended, a new occurrence waits for the keeper's recheck after the next update. An issue closed by hand, as fixed, wontfix or duplicate, stays closed and only records it.

A fault that comes back after a fix in a role or skill text gets a code barrier next: kebbie sets the issue's barrier to `code` itself, whatever the watcher answers, since a sentence that did not hold once will not hold the second time.

The fix task needs a checkout of kebbie's own repo in your worktree pool. Without one the issue waits for kebbie's maintainers, as any other.

## When a fault counts as fixed

The keeper's recheck after each `kebbie update` closes an issue once the installed kebbie holds its fix, as for every kebbie issue. Only then is it in use. An occurrence after that opens the issue again and counts as come back. `kebbie faults` lists each fault with how often it happened before and after.

## Your words stay here

The issue's title and text, which may go to kebbie's maintainers when you send them, describe kebbie alone. Your own words are kept only in `faults/occurrences/` in the state directory, shown by `kebbie faults show <id>`, and never sent.

## The lead keeps no memory of its own

kebbie refuses the lead writes to Claude Code's memory folder. A lesson about how kebbie or a lead works belongs in kebbie, where every lead after this one has it: the lead saves it as a kebbie issue, and the watcher queues its fix.

## Turning it off

The watcher follows `[issues] propose`: with `propose = false`, a machine that proposes no kebbie issues files none from your words either, and the lead starts the fix task for a fault you point out when kebbie's repo is among its areas. See [Report a problem with kebbie](../how-to/report-a-problem-with-kebbie.md).
