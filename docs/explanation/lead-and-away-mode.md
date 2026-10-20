# The lead and away mode

The lead is the Claude you talk to most. It runs in the lead Space next to the board. Feature work happens in each task's own Space; the lead starts it, steers it and checks it.

## Hiding the chat

`h` on the board in the lead Space hides the lead's chat, and `h` again shows it. The board then takes the whole width, which at 150 columns or more gives it the wide layout with the detail panel. Hiding is herdr's own pane zoom on the board: the chat keeps running unseen, at its size, and comes back as it was. The state is herdr's, so it outlives a board restart, and the board's status line says `chat hidden` while it lasts. `tab`, or `prefix+tab` from anywhere, shows the chat again and puts the focus there; going back to the board leaves it shown. `kelpie lead --toggle-chat`, or herdr's `kelpie.chat` action on a key, does the same as `h` from any Space and takes you to the lead. Moving herdr's focus onto the chat by other means while it is hidden zooms the chat instead, as herdr's zoom follows the focus.

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

## When the API stops a task

A task's Claude can stop because the API failed, not because the work is done: the API is overloaded, a server error, a rate limit, a timeout, a dropped connection, or the plan's usage limit. Claude Code retries some of these itself before it gives up. Once it has given up, it writes the error into the conversation (an assistant entry with `isApiErrorMessage`, its own `error` code and the HTTP status in `apiErrorStatus`) and the session sits idle. Away from the screen, that is a task that waits for nothing.

kelpie reads that entry and gets the task going again. Every 30 seconds the keeper, or an open board with `[keeper] enabled = false`, looks at each task whose Claude is idle and whose last answer is an API error nobody has typed after. Then:

- **Overload, a server error, a rate limit, a timeout or a dropped connection.** kelpie types `[from kelpie] … Carry on where you left off.` into the task 1, 2, 4 and 8 minutes after each error, one resume per error. The row says `retrying after overload (2/4, next 21:14)`. If the fifth error comes before Claude has answered once, the task stops.
- **The usage limit.** kelpie reads the reset from Claude Code's message, such as `You've hit your session limit · resets 5pm (Europe/Stockholm)`, and resumes the task a minute after it. The row says `out of allowance until 17:00`. A message without a reset kelpie can read takes the reset of the session or week window from the status line instead (`kelpie statusline`). With neither, the task goes to you.
- **A login, billing, invalid request or refusal error, or one kelpie does not know.** Waiting does not fix these. The task goes to Needs you at once with Claude Code's words, the lead gets a line, and `notify.command` gets an `api-error` event. The same happens to a task that kept failing through every retry.

kelpie never types into a draft: a resume waits while the input box holds text, and the delivery is checked as for any typed message. A reply from Claude ends the episode and the count starts again. A prompt from you does too, for a task that waits on you. Only tasks kelpie started are resumed, never the lead or your own Claude sessions. `[recover] enabled = false` turns it off.

## Restarting the lead

The lead reads its role and settings when it starts. `kelpie lead --restart` rewrites them and resumes the same conversation under the new role, so an update never costs you the lead's context. It waits until the lead is idle and not waiting on a question.

## Related

- [Work while you are away](../how-to/work-while-away.md)
- [Let the lead push](../how-to/let-the-lead-push.md)
