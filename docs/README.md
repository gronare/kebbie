# kelpie documentation

New to kelpie? Start with the tutorial. Come back to the how-to guides when you have something to get done, to the reference when you need an exact name, and to the explanations when you want to know why.

## Tutorial

- [Your first task](tutorial.md): install kelpie, run one task in its own worktree and push it.

## How-to guides

Tasks

- [Answer a task's questions](how-to/answer-questions.md): every waiting question in one window.
- [Review a task's changes](how-to/review-a-task.md): read the diff next to Claude and comment line by line.
- [Send a task to review](how-to/send-a-task-to-review.md): hold the push until a read-only reviewer passes the code.
- [Push part of a task](how-to/push-part-of-a-task.md): land the first commits and keep the task open.
- [Finish or throw away a task](how-to/finish-a-task.md): done, drop and reset.
- [Work across several repos](how-to/work-across-repos.md): one task with a half in each repo.
- [Move work into a task](how-to/move-work-into-a-task.md): take work out of the main checkout.

The lead

- [Let the lead push](how-to/let-the-lead-push.md): per task, or every reviewed task in a repo.
- [Work while you are away](how-to/work-while-away.md): away mode and the summary when you are back.
- [Get notifications](how-to/get-notifications.md): ntfy, macOS or any shell command.
- [Fill the backlog](how-to/fill-the-backlog.md): local file, vault and Linear.

Repos and worktrees

- [Set up a repo's worktrees](how-to/set-up-worktrees.md): `bin/setup-worktree` and the setup skill.
- [Run a task's dev server](how-to/run-dev-servers.md): `w`, `b` and the link on the board.
- [Use your own commit rules](how-to/use-your-own-commit-rules.md): replace the rules skill.
- [Add a release action to a repo](how-to/add-a-release-action.md): bump, tag and push from the Areas page.
- [Share a team setup](how-to/share-a-team-setup.md): repo configs linked in from a team repo.

The vault

- [Connect the vault](how-to/connect-the-vault.md): a note for every task and a log line for every push.
- [Run without the vault](how-to/run-without-the-vault.md): what to turn off and what you keep.

Your machine

- [Switch update channel](how-to/switch-update-channel.md): releases or every push to main.
- [Try unreleased code](how-to/try-unreleased-code.md): `--from` a branch or checkout, `--stable` back.
- [Turn off automatic restarts](how-to/turn-off-automatic-restarts.md): restart stale sessions yourself.
- [Show plan usage on the board](how-to/show-plan-usage.md): the status line command.
- [Show task state in herdr's sidebar](how-to/show-task-state-in-the-sidebar.md): the sidebar sync.
- [Fix common problems](how-to/troubleshoot.md): `kelpie doctor` and what each message means.

Working on kelpie

- [Develop kelpie](how-to/develop-kelpie.md): run kelpie from your own checkout.
- [Release kelpie](how-to/release-kelpie.md): bump, tag and publish.

## Reference

- [Commands](reference/commands.md): every `kelpie` command and option.
- [Configuration](reference/configuration.md): layers, every key, notify events and area actions.
- [Board](reference/board.md): layout, groups and every key, including the Areas page.
- [Review split](reference/review-split.md): its keys and behaviour.
- [Skills](reference/skills.md): the Claude Code plugin's skills and hooks.
- [Files and paths](reference/files.md): config, state, the backlog file and the setup note.
- [Environment variables](reference/environment.md): what kelpie reads and sets.
- [Requirements](reference/requirements.md): tools, versions and what `kelpie doctor` checks.

## Explanation

- [How kelpie fits together](explanation/architecture.md): the binary, the two plugins and the optional vault.
- [Trunk-based by design](explanation/trunk-based.md): why tasks land on the default branch with no pull request.
- [The review gate](explanation/review-gate.md): the lead's check, the read-only reviewer and what keeps a pass.
- [The worktree pool](explanation/worktree-pool.md): why worktrees are kept and reused.
- [The lead and away mode](explanation/lead-and-away-mode.md): what the lead holds and where it stops.
- [The trust boundary](explanation/trust-boundary.md): what kelpie guards against, and what it does not.
- [The vault](explanation/vault.md): what a task note adds.
- [Updates and stale sessions](explanation/updates-and-restarts.md): channels, overrides and restarts.
