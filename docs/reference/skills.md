# Skills

The skills the Claude Code plugin `kebbie@kebbie` ships, and the hooks kebbie gives a task. You run the skills marked with a slash command; kebbie or Claude runs the rest.

| Skill | Run by | What it does |
| --- | --- | --- |
| `kebbie:set-up-worktree` | A task's Claude at the start of a task, or `/kebbie:set-up-worktree` | Makes the worktree runnable without touching the main checkout. The default `worktree.setup_skill`. |
| `kebbie:work-rules` | A task's Claude when it commits, the reviewer, the lead, `tidy` | Commit rules. The default `task.rules_skill`. |
| `kebbie:review` | The reviewer started by `kebbie review`, and the rules check on a ready report | Reviews a task's branch independently and gives a verdict with located findings. |
| `kebbie:keep-docs` | A task's Claude in a repo with `docs.required`, or when a review finding ends `(keep-docs)` | Updates the README and `docs/` in the same task as the change. |
| `kebbie:configure` | Claude, when you ask it to configure kebbie | Changes a setting in config.toml or a repo config, sets a machine up, or adopts a team's linked setup, and checks each change with `kebbie doctor`. |
| `kebbie:using-kebbie` | Claude, the lead and tasks, when you ask about kebbie itself | Answers a question about kebbie from kebbie's own docs first, and cites the page the answer came from. |
| `kebbie:propose-change` | The lead, when kebbie cost something; a task's Claude, to tell the lead | Proposes saving what kebbie cost as a [kebbie issue](commands.md#kebbie-issues), at a calm moment. |
| `kebbie:review-split` | `/kebbie:review-split [base or what to review]` | Opens the [review split](review-split.md). |
| `kebbie:history-split` | `/kebbie:history-split` | Opens `tig` in a split next to the pane. |
| `kebbie:move-to-worktree` | `/kebbie:move-to-worktree <task name>` | Moves this session's work in progress out of the main checkout into its own worktree and Space, and continues there. |
| `kebbie:expose-dev-servers` | `/kebbie:expose-dev-servers` | On a remote Linux machine you reach over ssh, serves each worktree's dev server through Caddy at its own host, so it opens in your laptop's browser. |
| `kebbie:make-worktree-safe` | `/kebbie:make-worktree-safe` | Audits a repo for what breaks across worktrees and, on your yes, drafts `bin/setup-worktree` and the fixes in a worktree for you to review. |

`kebbie:set-up-worktree` sets up dependencies, the gitignored inputs the main checkout has, its own databases and a free port, then runs one check that tests run.

`kebbie:work-rules`:

- Its rules: steps in the order they go out, preparatory commits (a migration, an interface, a move, a rename) first and the change itself last, each one finished and pushable on its own; a subject and a body that says why, one logical change per commit, staging by explicit file list, the push left to you.
- The reviewer runs it in the rules check on a ready report and in `kebbie review`. The lead runs it when that check gives no verdict.

`kebbie:review`: the rules check on a ready report does only its commit, rules and docs steps.

`kebbie:keep-docs` finds what a change alters for the user and places each item on its page by Diátaxis (reference, how-to, explanation, tutorial).

`kebbie:using-kebbie` answers how to set something up, what a command, key or config does, or how a feature works. It reads [`kebbie docs <words>`](commands.md#setup-and-maintenance), the pages it names and `kebbie help`.

`kebbie:propose-change`:

- Unless `[issues] propose = false` keeps the lead to your own requests, it holds a line on what kebbie cost with `kebbie issue hold`.
- It proposes saving it at a calm moment, in a reply of its own once your request is done and nothing waits on you.
- It saves it on your yes, with a text about kebbie alone, and reopens a closed one that came back instead of filing it again.
- It never sends one anywhere or opens a GitHub issue for it. A bug reaches kebbie's maintainers through the hub report you approve (the bugs to send on the board, or `kebbie issue report`). `kebbie issue send` is yours to run, suggested only when you ask for a GitHub issue.

`kebbie:expose-dev-servers`:

1. Turns on `worktree.dev_route` per repo, on your yes.
2. Installs Caddy.
3. Writes `kebbie routes` to `/etc/caddy/kebbie.caddy` and reloads it.
4. Says what DNS or the proxy in front must point where.

## The review hook

The plugin's `PreToolUse` hook runs on `Edit`, `Write`, `MultiEdit` and `NotebookEdit`. It opens the review split on a task's first file edit when `auto_open` is on.

## Hooks kebbie gives a task

Each task's Claude starts with settings kebbie writes:

| Hook | Runs | For |
| --- | --- | --- |
| `SessionStart` | `kebbie hook inbox <task>` | Recording the session's inbox socket, which kebbie delivers its messages through. |
| `UserPromptSubmit`, `Stop` | `kebbie hook` | Delivering held messages and collecting the task's report at the end of a turn. |
| `PreToolUse` on `Edit`, `Write`, `MultiEdit`, `NotebookEdit`, `Bash` | `kebbie hook gate <task>` | With the vault only: holding edits until the task's note has its Problem. |
| `PreToolUse` on `Edit`, `Write`, `MultiEdit`, `NotebookEdit` | `kebbie hook files <task> …` | With the sandbox on only: keeping Claude's file tools to the task's worktrees and temp folder. |
| `PostToolUse`, `PostToolUseFailure` on `Bash` | `kebbie hook evidence <task>` | Recording the task's test runs for [`kebbie evidence`](commands.md#kebbie-evidence-name---json). It prints nothing. |

The same settings deny `git push`, every kebbie command that is yours alone or the lead's (such as `kebbie push`, `approve`, `stage`, `away`, `decide` and `issue send`), and herdr's commands that type into another pane.
