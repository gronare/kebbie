# Skills

The Claude Code plugin `kelpie@kelpie` ships these skills. You run the ones marked with a slash command; kelpie or Claude runs the rest.

| Skill | Run by | What it does |
| --- | --- | --- |
| `kelpie:set-up-worktree` | A task's Claude at the start of a task, or `/kelpie:set-up-worktree` | Makes the worktree runnable without touching the main checkout: dependencies, the gitignored inputs the main checkout has, its own databases, a free port, then one check that tests run. The default `worktree.setup_skill`. |
| `kelpie:work-rules` | A task's Claude when it commits, the reviewer (the rules check on a ready report and `kelpie review`), the lead when that check gives no verdict, `tidy` | Commit rules: a subject and a body that says why, one logical change per commit, staging by explicit file list, the push left to you. The default `task.rules_skill`. |
| `kelpie:review` | The reviewer started by `kelpie review`, and the rules check on a ready report, which does only its commit, rules and docs steps | Reviews a task's branch independently and gives a verdict with located findings. |
| `kelpie:keep-docs` | A task's Claude in a repo with `docs.required`, or when a review finding ends `(keep-docs)` | Finds what a change alters for the user, places each item on its page by Diátaxis (reference, how-to, explanation, tutorial), and updates the README and `docs/` in the same task. |
| `kelpie:configure` | Claude, when you ask it to configure kelpie | Changes a setting in config.toml or a repo config, sets a machine up, or adopts a team's linked setup, and checks each change with `kelpie doctor`. |
| `kelpie:using-kelpie` | Claude, the lead and tasks, when you ask about kelpie itself | Answers a question about kelpie (how to set something up, what a command, key or config does, how a feature works) from kelpie's own docs first: [`kelpie docs <words>`](commands.md#setup-and-maintenance), the pages it names and `kelpie help`, and cites the page the answer came from. |
| `kelpie:propose-change` | The lead, when kelpie cost something; a task's Claude, to tell the lead | Unless `[issues] propose = false` keeps the lead to your own requests, holds a line on it with `kelpie issue hold` and proposes saving it as a [kelpie issue](commands.md#kelpie-issues) at a calm moment, in a reply of its own once your request is done and nothing waits on you, saves it on your yes with a text about kelpie alone, reopens a closed one that came back instead of filing it again, and never sends one anywhere or opens a GitHub issue for it: a bug reaches kelpie's maintainers through the hub report you approve (`B` on the board, or `kelpie issue report`), and `kelpie issue send` is yours to run, suggested only when you ask for a GitHub issue. |
| `kelpie:review-split` | `/kelpie:review-split [base or what to review]` | Opens the [review split](review-split.md). |
| `kelpie:history-split` | `/kelpie:history-split` | Opens `tig` in a split next to the pane. |
| `kelpie:move-to-worktree` | `/kelpie:move-to-worktree <task name>` | Moves this session's work in progress out of the main checkout into its own worktree and Space, and continues there. |
| `kelpie:expose-dev-servers` | `/kelpie:expose-dev-servers` | On a remote Linux machine you reach over ssh, serves each worktree's dev server through Caddy at its own host so it opens in your laptop's browser: turns on `worktree.dev_route` per repo on your yes, installs Caddy, writes `kelpie routes` to `/etc/caddy/kelpie.caddy` and reloads it, and says what DNS or the proxy in front must point where. |
| `kelpie:make-worktree-safe` | `/kelpie:make-worktree-safe` | Audits a repo for what breaks across worktrees and, on your yes, drafts `bin/setup-worktree` and the fixes in a worktree for you to review. |

## The review hook

The plugin's `PreToolUse` hook runs on `Edit`, `Write`, `MultiEdit` and `NotebookEdit`. It opens the review split on a task's first file edit when `auto_open` is on.

## Hooks kelpie gives a task

Each task's Claude starts with settings kelpie writes:

| Hook | Runs | For |
| --- | --- | --- |
| `SessionStart` | `kelpie hook inbox <task>` | Recording the session's inbox socket, which kelpie delivers its messages through. |
| `UserPromptSubmit`, `Stop` | `kelpie hook` | Delivering held messages and collecting the task's report at the end of a turn. |
| `PreToolUse` on `Edit`, `Write`, `MultiEdit`, `NotebookEdit`, `Bash` | `kelpie hook gate <task>` | With the vault only: holding edits until the task's note has its Problem. |
| `PreToolUse` on `Edit`, `Write`, `MultiEdit`, `NotebookEdit` | `kelpie hook files <task> …` | With the sandbox on only: keeping Claude's file tools to the task's worktrees and temp folder. |
| `PostToolUse`, `PostToolUseFailure` on `Bash` | `kelpie hook evidence <task>` | Recording the task's test runs for [`kelpie evidence`](commands.md#kelpie-evidence-name---json). It prints nothing. |

The same settings deny `git push`, every kelpie command that is yours alone or the lead's (such as `kelpie push`, `approve`, `stage`, `away`, `decide` and `issue send`), and herdr's commands that type into another pane.
