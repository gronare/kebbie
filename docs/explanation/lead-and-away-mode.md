# The lead and away mode

The lead is the Claude you talk to most. It runs in the lead Space next to the board. Feature work happens in each task's own Space; the lead starts it, steers it and checks it.

## Why a lead

With one Claude, you are the one holding the picture. With five, you cannot: you would spend your time switching Spaces, reading scrollback and copying what one session said into another. The lead takes that job. It holds the tasks, repos, plans and decisions, so you can come back after an hour and ask "what needs me?"

Concretely, the lead:

- **briefs you** when you arrive: what needs you first, then what is working, to review and ready to push, each with a recommendation,
- **finds work** in your backlog sources and offers two or three candidates,
- **starts tasks** with informed prompts: the ticket, the notes and decisions that bear on it, what other tasks touch, and what done looks like,
- **holds tasks to your rules** by checking a ready task's commits against the rules skill, and approving them or sending back what to fix,
- **relays** interface and contract changes between tasks, and brings you what needs a decision,
- **does small cross-cutting fixes** itself, such as a script or a rename across repos.

It does not push, except tasks you gave it permission for.

## Why it sees everything without being flooded

Every report, message and question passes through kelpie, and the lead gets a copy. The copies do not arrive as chat messages. Each of your prompts to the lead carries the reports and messages it has not seen yet, so its chat stays a conversation with you and its picture stays current.

Questions are different: they are yours. A task asks with `kelpie ask`, and the question waits on the board until you answer it there. The lead can help you decide, but the answer comes from you, and it reaches the task as `[from you]`. Until you answer, kelpie holds every other message to that task, so nothing starts a turn under an open question.

## Away mode

Some work should continue while you are gone. Away mode lets the lead keep tasks moving without deciding things that are yours.

The line it draws is between **how** and **what**:

- A question about method, such as which library, which structure or what order, the lead settles with `kelpie decide`. kelpie passes the answer to the task as the lead's call and records it.
- A question about intent, such as scope, behaviour, a trade-off that is yours or a plan to approve, the lead keeps for you with `kelpie defer`.
- The lead pushes only what your push permissions allow.

When you switch away mode off, kelpie hands the lead what was pushed, what it decided and the questions it kept, and the lead writes you a summary with the kept questions first. With a `notify.command`, you hear about landings and kept questions while you are away.

Only you can switch away mode. A task or a message cannot turn it on for you. See [The trust boundary](trust-boundary.md).

## Restarting the lead

The lead reads its role and settings when it starts. `kelpie lead --restart` rewrites them and resumes the same conversation under the new role, so an update never costs you the lead's context. It waits until the lead is idle and not waiting on a question.

## Related

- [Work while you are away](../how-to/work-while-away.md)
- [Let the lead push](../how-to/let-the-lead-push.md)
