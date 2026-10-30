# kebbie documentation

Start with the tutorial. Use the how-to guides to get something done, the reference for an exact name, and the explanations for why.

With kebbie installed, `kebbie docs <words>` finds the page.

## Tutorial

- [Your first task](tutorial.md): install kebbie, run one task in its own worktree and push it.

## How-to guides

Tasks

- [Answer a task's questions](how-to/answer-questions.md): every waiting question in one window.
- [Review a task's changes](how-to/review-a-task.md): read the diff next to Claude and comment line by line.
- [Send a task to review](how-to/send-a-task-to-review.md): hold the push until a read-only reviewer passes the code.
- [Push part of a task](how-to/push-part-of-a-task.md): land the first commits and keep the task open.
- [Work in steps](how-to/work-in-steps.md): a task lists its steps, you accept and push them one at a time.
- [Try a task before pushing](how-to/try-a-task-before-pushing.md): put its commits on local main, push main when it is right.
- [Finish or throw away a task](how-to/finish-a-task.md): done, drop and reset.
- [Park a task until later](how-to/park-a-task.md): out of its group, the lead's summaries and your notifications until it comes back.
- [Work across several repos](how-to/work-across-repos.md): one task with a half in each repo.
- [Move work into a task](how-to/move-work-into-a-task.md): take work out of the main checkout.

The lead

- [Let the lead push](how-to/let-the-lead-push.md): per task, or every reviewed task in a repo.
- [Work while you are away](how-to/work-while-away.md): away mode and the summary when you are back.
- [Get notifications](how-to/get-notifications.md): ntfy, macOS or any shell command.
- [Fill the backlog](how-to/fill-the-backlog.md): local file, vault and Linear.
- [Add a routine](how-to/add-a-routine.md): a check kebbie runs on a schedule as a task.

Repos and worktrees

- [Set up a repo's worktrees](how-to/set-up-worktrees.md): `bin/setup-worktree` and the setup skill.
- [Run a task's dev server](how-to/run-dev-servers.md): `w`, `b` and the link on the board.
- [Open dev servers from a remote machine](how-to/open-dev-servers-from-a-remote-machine.md): one host per worktree through Caddy, opt-in per repo.
- [Use your own commit rules](how-to/use-your-own-commit-rules.md): replace the rules skill.
- [Keep docs in step with changes](how-to/keep-docs-in-step.md): the reviewer fails a change to what users see that documents nothing.
- [Decide what a task does with what it notices](how-to/decide-what-a-task-does-with-what-it-notices.md): file it, fix it or ask you.
- [Let tasks run without permission prompts](how-to/let-tasks-run-without-prompts.md): bypass permissions mode, on an isolated machine.
- [Give tasks a profile](how-to/give-tasks-a-profile.md): a model, role, skills and permissions for one kind of work.
- [Run a task with Codex](how-to/run-a-task-with-codex.md): a profile with `agent = "codex"` and kebbie's Codex plugin.
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
- [See your board on your phone](how-to/see-your-board-on-your-phone.md): connect a kebbie hub and read the board on any phone you sign in on.
- [Fix common problems](how-to/troubleshoot.md): `kebbie doctor` and what each message means.
- [Report a problem with kebbie](how-to/report-a-problem-with-kebbie.md): save it on the machine, send it to kebbie's maintainers, and see it rechecked after an update.

Working on kebbie

- [Develop kebbie](how-to/develop-kebbie.md): run kebbie from your own checkout.
- [Receive bug reports as a maintainer](how-to/receive-bug-reports-as-a-maintainer.md): the reports users send and the error signals three installs share, filed as kebbie issues.
- [Release kebbie](how-to/release-kebbie.md): bump, tag and publish.

## Reference

- [Commands](reference/commands.md): every `kebbie` command and option.
- [Configuration](reference/configuration.md): layers, every key, notify events and area actions.
- [Board](reference/board.md): layout, groups and every key, including the Areas page.
- [Review split](reference/review-split.md): its keys and behaviour.
- [Skills](reference/skills.md): the Claude Code plugin's skills and hooks.
- [Files and paths](reference/files.md): config, state, the backlog file and the setup note.
- [Environment variables](reference/environment.md): what kebbie reads and sets.
- [Requirements](reference/requirements.md): tools, versions and what `kebbie doctor` checks.

## Explanation

- [How kebbie fits together](explanation/architecture.md): the binary, the keeper, the two plugins, and the optional vault and hub.
- [Trunk-based by design](explanation/trunk-based.md): why tasks land on the default branch with no pull request.
- [The review gate](explanation/review-gate.md): the rules check, the read-only reviewer and what keeps a pass.
- [The worktree pool](explanation/worktree-pool.md): why worktrees are kept and reused.
- [The lead and away mode](explanation/lead-and-away-mode.md): what the lead holds, where it stops, and how it restarts.
- [Routines](explanation/routines.md): scheduled checks that report, plan or fix.
- [The trust boundary](explanation/trust-boundary.md): what kebbie guards against, and what it does not.
- [The vault](explanation/vault.md): what a task note adds, and two machines sharing one vault.
- [Updates and stale sessions](explanation/updates-and-restarts.md): channels, overrides and restarts.
