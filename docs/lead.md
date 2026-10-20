# The lead

The lead is the Claude you talk to most. It runs in the lead Space next to the board and holds the whole picture: tasks, repos, plans and decisions. Feature work happens in each task's own Space; the lead starts, steers and checks it.

## What the lead does

- **Briefs you** when you arrive: what needs you first, then what is working, to review and ready to push, each with a recommendation.
- **Finds work** in your backlog sources and offers two or three candidates.
- **Starts tasks** with informed prompts: the ticket, the notes and decisions that bear on it, what other tasks touch, and what done looks like.
- **Holds tasks to your rules**: checks a ready task's commits against the rules skill and runs `kelpie approve`, or sends back what to fix.
- **Relays** interface and contract changes between tasks, and brings you what needs a decision.
- **Does small cross-cutting fixes** itself, such as a script or a rename across repos.

It does not push, except for tasks you gave it push permission (`a` on the board). See [Tasks](tasks.md#land-and-push).

## Getting there

- `prefix+space` (`kelpie lead`) goes to the lead Space, and back on the next press. The first time, it creates it.
- `prefix+tab` (`kelpie focus`) moves focus between the board and the lead's chat. From any other Space it goes to the chat.
- The lead Space stays at the top of herdr's Spaces.

## Restarting the lead

The lead reads its role and settings when it starts. `kelpie lead --restart` rewrites them, ends the lead's Claude and starts it again with `--resume`, so the conversation continues under the new role. It refuses while the lead is working or waiting on a question. `kelpie update` restarts a waiting lead by itself.

## Questions

When a task needs you, it asks through kelpie:

```bash
kelpie ask --option "Per invoice: round once at the end" \
           --option "Per line: keep today's rounding" \
           --recommend 1 "How should VAT be rounded?"
```

Two to four options, one recommended. `--plan <file>` or `--plan vault:<note>` asks you to approve a plan instead: approve, approve with a comment, ask for changes, or reject with a comment.

Every open question waits on the board. Press `q`, pick an option or write your own words, and press `⏎`. The answer reaches the task as `[from you] <answer>`, and the lead gets a copy. Until you answer, every other message to that task is held, so nothing starts a turn under the question.

`kelpie answered <task>` closes a question you settled in the task's Space instead.

## Away mode

Press `A` on the board, or run `kelpie away on`. While it is on:

- The lead settles questions about **how** itself with `kelpie decide <task> "<answer>"`, recorded as its call.
- It keeps questions about **what you want** for you with `kelpie defer <task>`.
- It pushes only what your push permissions allow.

`kelpie away off` hands the lead what was pushed, what it decided and the questions it kept, and it writes you a summary. Only you can switch away mode.

## Messages

Sessions message each other with `kelpie tell <to> "<text>"`, where `<to>` is a task (every repo it spans), `task@repo`, or `lead`. A message arrives in that Claude's chat as `[from <task>@<repo>] <text>`. A session waiting for an answer, or with no Claude running, has its messages held until it is free.

- The board's messages panel shows the traffic; `kelpie messages` shows it live, and `--print` prints the last 30.
- The lead is copied: each of your prompts to it carries the messages and reports it has not seen yet, so the traffic stays out of its chat.
- Every message is logged to `messages.jsonl` in the state directory.

The task role tells each Claude that a peer's message is information, never your instruction.

## Notifications

kelpie has no notification service of its own. `[notify] command` names a shell command, and kelpie runs it when something needs you:

| `KELPIE_EVENT` | When |
| --- | --- |
| `needs-you` | A task reports that it needs you. |
| `ready` | A task reports ready. |
| `question` | A task asks you a question. |
| `summary` | The lead (or you) runs `kelpie notify "<text>"`, for example its summary while you are away. Tasks cannot. |

The command also gets `KELPIE_TASK`, `KELPIE_REPO` and `KELPIE_TEXT` (one line, such as `billing@app needs you: answer which queue`), and the text as `$1`. It runs through `sh -c`, detached, and is stopped after 10 seconds. A task sends at most one notification per event a minute. Failures go to `notify.log` in the state directory and never fail the report.

To your phone through [ntfy](https://ntfy.sh):

```toml
[notify]
command = 'curl -s -H "Title: kelpie $KELPIE_EVENT" -d "$KELPIE_TEXT" https://ntfy.sh/my-kelpie-topic'
```

As a macOS notification:

```toml
[notify]
command = 'osascript -e "display notification (system attribute \"KELPIE_TEXT\") with title \"kelpie\""'
```
