# Skills

The Claude Code plugin `kelpie@kelpie` ships these skills. You run the ones marked with a slash command; kelpie or Claude runs the rest.

| Skill | Run by | What it does |
| --- | --- | --- |
| `kelpie:set-up-worktree` | A task's Claude at the start of a task, or `/kelpie:set-up-worktree` | Makes the worktree runnable without touching the main checkout: dependencies, the gitignored inputs the main checkout has, its own databases, a free port, then one check that tests run. The default `worktree.setup_skill`. |
| `kelpie:work-rules` | A task's Claude when it commits, the lead when it checks, `tidy`, the reviewer | Commit rules: a subject and a body that says why, one logical change per commit, staging by explicit file list, the push left to you. The default `task.rules_skill`. |
| `kelpie:review` | The reviewer started by `kelpie review` | Reviews a task's branch independently and gives a verdict with located findings. |
| `kelpie:configure` | Claude, when you ask it to configure kelpie | Changes a setting in config.toml or a repo config, sets a machine up, or adopts a team's linked setup, and checks each change with `kelpie doctor`. |
| `kelpie:review-split` | `/kelpie:review-split [base or what to review]` | Opens the [review split](review-split.md). |
| `kelpie:history-split` | `/kelpie:history-split` | Opens `tig` in a split next to the pane. |
| `kelpie:move-to-worktree` | `/kelpie:move-to-worktree <task name>` | Moves this session's work in progress out of the main checkout into its own worktree and Space, and continues there. |
| `kelpie:make-worktree-safe` | `/kelpie:make-worktree-safe` | Audits a repo for what breaks across worktrees and, on your yes, drafts `bin/setup-worktree` and the fixes in a worktree for you to review. |

## The review hook

The plugin's `PreToolUse` hook runs on `Edit`, `Write`, `MultiEdit` and `NotebookEdit`. It opens the review split on a task's first file edit when `auto_open` is on.

## Hooks kelpie gives a task

Each task's Claude starts with settings kelpie writes:

| Hook | Runs | For |
| --- | --- | --- |
| `UserPromptSubmit`, `Stop` | `kelpie hook` | Delivering held messages and collecting the task's report at the end of a turn. |
| `PreToolUse` on `Edit`, `Write`, `MultiEdit`, `NotebookEdit`, `Bash` | `kelpie hook gate <task>` | With the vault only: holding edits until the task's note has its Problem. |
| `PostToolUse`, `PostToolUseFailure` on `Bash` | `kelpie hook evidence <task>` | Recording the task's test runs for [`kelpie evidence`](commands.md#kelpie-evidence-name---json). It prints nothing. |

The same settings deny `git push`, `kelpie approve` and `kelpie push`.
