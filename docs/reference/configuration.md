# Configuration

Every key kebbie reads, the file it goes in and what it does. Every key has a default, so kebbie runs without a config file. `kebbie config` prints the keys with the value in effect, the layer it comes from and what it does, and the board's [Settings page](board.md#settings-page) (`,`, or `kebbie config edit`) changes them in place.

## Files and layers

kebbie reads each key from the first layer that sets it:

| Layer | File | Holds |
| --- | --- | --- |
| Repo config | `~/.config/kebbie/repos/<repo>.toml` | `[worktree]`, `[task]`, `[review]`, `[docs]`, `[[actions]]` and `[[routines]]` for one repo. |
| Machine config | `~/.config/kebbie/config.toml` | Every key. |
| Defaults | built in | Every key. |

- `<repo>` is the main checkout's folder name, so every worktree of `~/code/app` reads `repos/app.toml`.
- [Profiles](#profiles) are files of their own, one per profile in `~/.config/kebbie/profiles/`.
- `$XDG_CONFIG_HOME/kebbie` replaces `~/.config/kebbie` when it is set. `KEBBIE_CONFIG`, `KEBBIE_REPOS_DIR` and `KEBBIE_PROFILES_DIR` override each path; see [Environment](environment.md).
- kebbie never reads settings from a file inside a repo.

Rules:

- A key marked **config.toml only** in a repo config is an error that says so.
- An unknown key or table in either file is ignored, with one line on stderr that names the file and the key, so a key from a newer kebbie never stops this one. `kebbie doctor` lists them as warnings.
- A known key with a value of the wrong type is an error that names the file and the key.
- Unknown keys inside an `[[actions]]` or `[[routines]]` entry are errors, since that entry says what kebbie runs.
- `kebbie update` reads only `update.channel` from config.toml, so nothing else in the file stops an update.
- Either file may be a symlink. `kebbie config` and `kebbie doctor` show each link with its target, and `kebbie doctor` reports a broken one.

## Keys

The Where column says which file may set a key: `either`, `config.toml only` or `repo config only`. A key's details follow its group's table.

### Board

| Key | Default | Where | What it does |
| --- | --- | --- | --- |
| `board.backlog.sources` | `["local"]` | config.toml only | Where the Backlog column finds work and new items are written: `"local"` or `"vault"`, and `"linear"` beside either. See [Backlog sources](#backlog-sources). |
| `board.backlog.ongoing` | `"top"` | config.toml only | Where the Backlog column shows ONGOING, the vault notes in progress: `"top"`, `"bottom"` or `"hidden"`. |
| `board.backlog.template` | none | config.toml only | A file with the sections of local backlog items. See [Backlog sources](#backlog-sources). |
| `board.groups` | the order below | config.toml only | The order of the Work column's groups. |
| `[keys]` | the board's own | config.toml only | The board's keys by action, a table of strings, as `inbox = "I"`. See [Board](board.md#keys-in-configtoml). |

- `board.backlog.sources`: new items are written to `"local"` or `"vault"`, so a list holds one of them; Linear is only read. A config that lists both reads as the vault alone, and `kebbie doctor` says so. `[]` turns the backlog off.
- `board.backlog.ongoing`: above the backlog items, below them, or not at all. Hidden, a note another machine claims still asks before it starts here.
- `board.backlog.template`: a JSON or TOML file, read from the folder of config.toml when the path is relative. Unset, local items have kebbie's own sections: Background, Goal and Source required, What we know, Open questions and Out of scope beside them. Vault items follow the vault's task template, and kebbie's own when the vault has none. `kebbie backlog template --local` shows the one in use.

  ```toml
  # ~/.config/kebbie/item.toml
  questions = "risks"

  [[sections]]
  key = "problem"
  heading = "Problem"
  required = true

  [[sections]]
  key = "risks"
  heading = "Risks"
  ```
- `board.groups`: a list of `"needs-you"`, `"working"`, `"in-review"`, `"ready-to-push"`, `"ci-running"`, `"waiting-for-work"`, `"queued"`, by default in that order. Groups the list leaves out follow in the default order, so `["ready-to-push"]` moves only Ready to push to the top. A name not in the list is an error that names the valid ones.
- `[keys]`: [Board](board.md#keys-in-configtoml) has the actions and the rules. `kebbie config` shows each key in effect and why an entry was ignored.

### Worktrees

| Key | Default | Where | What it does |
| --- | --- | --- | --- |
| `worktree.setup_skill` | `"kebbie:set-up-worktree"` | either | The skill a task's Claude runs first to make its worktree runnable, or a Markdown file of steps. |
| `worktree.setup_command` | none | either | The repo's own command that prepares a worktree, run from inside it by the setup skill. |
| `worktree.dev_server` | none | either | The command the board's `w` runs in a server tab of the task's Space. Unset, there is no `w`. |
| `worktree.dev_worker` | none | either | A second command, such as a job runner, that `b` runs in a worker tab. Unset, there is no `b`. |
| `worktree.dev_url` | `"http://localhost:{port}"` | either | The running dev server's address, shown on the board as a link. `""` turns the link off. |
| `worktree.dev_route` | `false` | repo config only | Whether [`kebbie routes`](commands.md#setup-and-maintenance) writes a Caddy site for each of this repo's worktrees. |
| `worktree.stage_on_main` | `false` | repo config only | Whether [`kebbie stage`](commands.md#kebbie-stage-name) (`M` on the board) may put a ready task's commits on this repo's local default branch. |
| `worktree.clean_on_release` | `["target"]` | either | Build output kebbie removes from a worktree when it goes back to the pool. `[]` keeps everything. |
| `worktree.server_name` | `"server"` | config.toml only | The name of the tab `w` starts, and of the Space's tag in herdr's sidebar while it runs. |
| `worktree.worker_name` | `"worker"` | config.toml only | The same for `b`'s tab. |

- `worktree.setup_skill`: `plugin:skill` for a plugin's skill; a bare name for one in `~/.claude/skills` or the repo's `.claude/skills`; or a Markdown file by an absolute or `~` path, such as `"~/code/app/SETUP.md"`, whose text the task gets in its role.
- `worktree.setup_command`: a command naming a path applies only in a repo that has that file. Where it does not apply, and when unset, the skill runs `bin/setup-worktree` if the repo has one.
- `worktree.dev_server` and `worktree.dev_worker` run with `PORT` set to the worktree's port.
- `worktree.dev_url` fills in the [URL placeholders](#url-placeholders).
- `worktree.dev_route`: each site is at the host `dev_url` names, proxied to the worktree's port. It is opt-in per repo, so a repo that serves its worktrees itself stays out. See [Open dev servers from a remote machine](../how-to/open-dev-servers-from-a-remote-machine.md).
- `worktree.stage_on_main`: the commits stay unpushed, to try them from the main checkout before you push main. It is for a repo you run from its main checkout; kebbie's own repo, when you run kebbie from your clone, turns it on. See [Try a task before pushing](../how-to/try-a-task-before-pushing.md).
- `worktree.clean_on_release` runs on `done`, `drop`, `reset` and a landed task the keeper closes, so the pool's disk use stays bounded. The result says what it freed, as `freed: removed target/ (3.7 GB)`. Its entries are folders inside the worktree, such as `"node_modules/.cache"`, `".next"` or `"dist"`. Each is removed only when git ignores it and tracks nothing in it, a symlink is never followed, and `"target"` only where the worktree has a `Cargo.toml`.

### Tasks

| Key | Default | Where | What it does |
| --- | --- | --- | --- |
| `task.rules_skill` | `"kebbie:work-rules"` | either | The skill with the rules for how work is planned, built and committed, or a Markdown file of rules. |
| `task.test_commands` | `[]` | either | Test commands kebbie records when a task's Claude runs them, on top of the built-in ones. |
| `task.also_noticed` | `"backlog"` | either | What a task's Claude does with what it notices outside its task: `"backlog"`, `"fix"` or `"ask"`. |
| `task.profile` | none | either | The [profile](#profiles) a task starts with when `kebbie start` names none with `--profile`. |
| `task.sandbox` | `true` | either | Whether a task's and the reviewer's shell commands run in Claude Code's sandbox. |
| `task.steps` | `true` | either | Whether tasks work in steps, each a slice of the work in small commits, listed with `kebbie steps` before they write code. |
| `task.require_accept` | `false` | either | Whether every commit must be accepted before it can be pushed. Only with `task.steps` on. |
| `task.permission_mode` | none | either | The [permission mode](https://code.claude.com/docs/en/permission-modes) a task's Claude starts in. |
| `[[routines]]` | none | either | Checks kebbie runs on a schedule, each as a task that reports, plans or fixes. See [`[[routines]]`](#routines). |

`task.rules_skill`: a task's Claude follows it, the reviewer checks a ready task against it, and `tidy` reshapes commits by it. It names a skill as `worktree.setup_skill` does, or a Markdown file by an absolute or `~` path, such as `"~/code/team/RULES.md"`: the task gets the file's text in its role and the reviewer in its brief. With the default, kebbie also refuses a ready report while a commit lacks a subject, a blank line and a body.

`task.test_commands`: the built-in ones are `cargo test`, `bin/rails test`, `pytest`, `npm test`, `go test` and others; `kebbie config` lists them. A command counts when one of these, whole words, starts it or any part of its `;`, `&&` or `||` chain, after `cd`, `export`, `VAR=value` and wrappers such as `timeout` or `nice`. See [`kebbie evidence`](commands.md#kebbie-evidence-name---json).

`task.also_noticed` covers things such as a bug nearby, a stale doc or a refactor. See [Decide what a task does with what it notices](../how-to/decide-what-a-task-does-with-what-it-notices.md).

- `"backlog"` writes it out as a [backlog item](../how-to/fill-the-backlog.md#what-a-written-out-item-holds) and files it where new items go: in the vault's backlog, or with `kebbie backlog add`.
- `"fix"` fixes a small one beside the code it is changing, in a commit of its own, and files the rest.
- `"ask"` puts each to you with `kebbie ask` first.
- Each is listed under Also noticed on the sign-off card. Under `"backlog"` and `"ask"`, the reviewer counts a change outside the task as a finding.

`task.profile`: unset, tasks start with the built-in `task`. `"none"` in a repo's file starts its tasks with the built-in `task` whatever config.toml names. A restarted task keeps the profile it started with. The board's Settings page picks it from the profiles there are, each with its `for`, and `none (task)` removes the key. See [Give tasks a profile](../how-to/give-tasks-a-profile.md).

`task.steps`: on, the default, a task's role tells it to list its steps with `kebbie steps` before its first line of code, marks each one ready with `kebbie steps done`, and you accept and push its steps one at a time while it builds the next. `false`, in the machine config or in a repo's own file, turns it off: a task commits its own way and `kebbie steps` refuses. New tasks follow it; a running task once it restarts. `kebbie accept` works either way. See [Work in steps](../how-to/work-in-steps.md).

`task.require_accept`: off, the default, accepting a step is up to you and a push takes what is committed. `true`, in the machine config or in a repo's own file, holds every push until each commit it takes is accepted: `kebbie push`, `--through`, the board's push and push in order refuse with the first step not accepted and how to accept it, and a ready task with a step to accept waits under Needs you, `accept step N` under its row. With `task.steps` off it has no effect. See [Require every step accepted](../how-to/work-in-steps.md#require-every-step-accepted).

`task.sandbox` is strict: no retry outside it, and no start without it. See [The trust boundary](../explanation/trust-boundary.md#the-sandbox).

- A task's shell writes only in its worktree, its temp folder, the package caches (`~/.cargo/registry`, `~/.cargo/git`, `~/.npm`, `~/.cache`) and the repo's shared `.git` folder, except its hooks, config and the other pooled worktrees' git dirs.
- It reaches only crates.io, static.crates.io, index.crates.io, registry.npmjs.org, github.com, objects.githubusercontent.com, pypi.org and files.pythonhosted.org, and no Unix socket.
- On macOS it may listen on local ports for a dev server.
- kebbie's own task commands (`peers`, `tell`, `ask`, `report`, `steps`, `routine ok`, `link`, `backlog add`, `config`) run outside it.
- On Linux it needs bubblewrap, socat and the seccomp filter (`npm install -g @anthropic-ai/sandbox-runtime`). Without them kebbie refuses to start a task or a review and names what to install.
- `false` runs them unsandboxed, where a task's shell can pass as you to kebbie's user-only commands. `kebbie doctor`, a task's start and the board's Areas page say so.
- A profile's `[sandbox] enabled` wins over it.

`task.permission_mode` is one of `"default"`, `"acceptEdits"`, `"plan"`, `"auto"`, `"dontAsk"` or `"bypassPermissions"`. See [Let tasks run without permission prompts](../how-to/let-tasks-run-without-prompts.md).

- Unset, Claude Code's own default applies.
- `kebbie start --mode` and then the task's profile's `permission_mode` override it, and a restarted task keeps the mode it started in.
- Any other value is refused when the config loads.
- `"bypassPermissions"` runs every tool without asking; Claude Code recommends it only for an isolated container or VM. A task still never pushes nor approves its own commits in it, since kebbie's deny rules and the worktree's push block hold in every mode.

### Review and docs

| Key | Default | Where | What it does |
| --- | --- | --- | --- |
| `docs.required` | `false` | repo config only | Whether this repo's docs keep in step with its code. |
| `review.profile` | `"reviewer"` | either | The review profile that reviews by default; its verdict holds the push. |
| `review.second` | none | either | A second opinion: a review profile offered with `s` in the board's `v` menu, or `"always <profile>"` to run it with every review. It never holds the push. |
| `review.model` | none | either | The model the reviewer `review.profile` names runs on. |
| `review.auto` | `"suggest"` | either | What kebbie does with work the assessor says needs a review: `"suggest"`, `"run"` or `"require"`. |

`docs.required`: a task's Claude updates the README and `docs/` when its change alters what a user sees or does, by the [`kebbie:keep-docs`](skills.md) skill. The reviewer, in the rules check on a ready report and in `kebbie review`, fails a branch that changes such behaviour and leaves it undescribed in the README or `docs/`. `kebbie doctor` suggests it for a repo that is public on GitHub. See [Keep docs in step with changes](../how-to/keep-docs-in-step.md).

`review.profile` names a review profile, one with `kind = "review"` in `~/.config/kebbie/profiles/`, or the built-in `reviewer`, which `"none"` also names. A task's profile is refused. The Settings page picks from the review profiles whose agent you set up. See [Add a review profile](../how-to/give-tasks-a-profile.md#add-a-review-profile).

`review.second` names another review profile. Its points show on the task's row and go to the lead, which passes them to the task as a review's findings; the push never waits on them. Set, `s` (review with a second opinion) and `o` (the second opinion only) join the board's `v` menu, and `kebbie review --second` and `--second-only` run it. `"always <profile>"` runs it with every review, `v` included. `"none"` turns off a second opinion config.toml sets.

`review.model` is the `model` of the profile `review.profile` names, by another name, and ahead of it when both are set. Unset leaves it to that profile, and then to its agent's default.

`review.auto`: on a ready report, a read-only assessor judges whether the task's work since it started, pushed steps included, needs an independent review.

- `"suggest"`: the row and the lead say `review suggested: <why>`, and nothing starts until you or the lead send it.
- `"run"`: the reviewer starts by itself; the push is not held.
- `"require"`: the push is held from the ready report on, until the reviewer passes.

An assessment with no answer reads as `review not assessed`, a suggestion in every mode. A task you withdraw from review is not assessed again until you send it. See [An assessor judges whether the work needs a review](../explanation/review-gate.md#an-assessor-judges-whether-the-work-needs-a-review).

### Investigation

| Key | Default | Where | What it does |
| --- | --- | --- | --- |
| `investigate.profile` | `"investigator"` | either | The profile kebbie's investigator runs with on an obstacle or a review's finding about scope or intent; `"none"` runs none. |

`investigate.profile` names any profile in `~/.config/kebbie/profiles/` or a built-in one, and the investigator takes its agent, model, effort and role: a Claude profile with its model and effort, or a Codex one. The built-in `investigator` is Claude at high effort, with your own settings, plugins and MCP servers. With `"none"`, a raised obstacle goes to the lead, which decides it with [`kebbie obstacles decide`](commands.md#kebbie-obstacles-name---json). See [Obstacles and findings](../explanation/obstacles-and-findings.md#one-investigation-per-obstacle).

```toml
[investigate]
profile = "deep-codex"
```

### Machine

| Key | Default | Where | What it does |
| --- | --- | --- | --- |
| `machine.name` | the host's short name | config.toml only | The machine's one name, on your phone through the hub and on the vault notes it claims. |
| `notify.command` | none | config.toml only | The shell command kebbie runs when something needs you. Unset sends nothing. See [Notify command](#notify-command). |
| `vault.command` | none | config.toml only | The vault command kebbie runs for task notes, log lines and the board's snapshot, before its own arguments, such as `"uvx mindin"`. |
| `hub.url` | the kebbie hub, `https://hub.kebbie.dev` | config.toml only | The [kebbie hub](../how-to/see-your-board-on-your-phone.md) `kebbie hub connect` and the board's `H` window connect this machine to. |
| `hub.commands` | all seven kinds | config.toml only | What this machine takes from your devices through the hub: a list of `"answer"`, `"push"`, `"prompt"`, `"idea"`, `"start"`, `"edit"` and `"review"`. See [Answer and push from your phone](../how-to/see-your-board-on-your-phone.md#answer-and-push-from-your-phone). |
| `hub.trust` | `"device"` | config.toml only | Which of your devices this machine takes commands from: `"device"`, each device you approve here, or `"account"`, any device signed in to your account. |
| `hub.auto_approve` | `"24h"` | config.toml only | When a device waiting under `hub.trust = "device"` is let in: `"24h"` after its first command unless you deny it, or `"never"`, only when you approve it. |
| `hub.approve_from` | `"devices"` | config.toml only | Where a waiting device can be approved: `"devices"`, also from a device you approved on this machine, or `"machine"`, only here. |

`machine.name`: your phone shows it through the [hub](../how-to/see-your-board-on-your-phone.md#connect-the-machine), and kebbie claims vault notes under it. See [Two machines, one vault](../explanation/vault.md#two-machines-one-vault).

- `kebbie hub connect`, the board's `H` window and `kebbie hub join` use it, set it from `--name`, or ask for it when it is unset. [`kebbie:configure`](skills.md) asks for it first when it sets kebbie up.
- A change renames the machine on the hub with the keeper's next post, or at once with `kebbie hub status`. While that fails, `kebbie hub status` and `kebbie doctor` say why.
- At most 64 characters, without control characters. `"localhost"` is refused, and `kebbie doctor` warns while the key is unset.
- A task started from a vault note, or one kebbie opens a note for, writes this name and the task's into the note's `kebbie` frontmatter key. kebbie on another machine sharing the vault shows the note as `working on <name> · <task>` and asks before starting it.
- Give each machine its own stable name, such as `"mac"` and `"devbox"`.

`vault.command` is kebbie's own way to the vault, whatever agent runs the task: Claude Code, Codex or another.

- [`kebbie setup`](commands.md#kebbie-setup) writes the vault command it runs the vault with, chosen from the ones on this machine. See [Setup picks the command](../explanation/vault.md#setup-picks-the-command).
- The command and its leading words, split at spaces: `"uvx mindin"`, `"uvx mindin==0.14.0"`, or a checkout of your own, `"uv run --project ~/code/mindin mindin"`.
- kebbie asks it what it is with `about --json` and uses only the capabilities it states. See [The vault contract](vault-contract.md).
- Unset, kebbie runs the Claude vault plugin's pinned command from its hooks while the plugin is installed and enabled, and `kebbie doctor` suggests the line to set.

`hub.url` is `https`, or `http` on this machine.

- Unset, `connect` and `H` ask whether to use the kebbie hub or your own. Choosing your own tests the address and sets this key, never through a config.toml that is a link.
- The hub a machine is connected to stays with its keys in [`hub/`](files.md#the-state-directory), so a change here moves nothing. `kebbie hub move` moves it, and so does a permanent redirect (308) to another port or to `https` on the same host.
- `kebbie config` shows it, and `hub.connected` with the hub this machine posts to.

`hub.commands` lists the kinds of command this machine takes from your devices through the hub. A command counts only from a device `hub.trust` lets in, below.

| Kind | What the machine takes |
| --- | --- |
| `"answer"` | Answers to a task's questions and to a backlog item's open questions. |
| `"push"` | Pushes of a task. |
| `"prompt"` | Choices on an agent's prompt. |
| `"idea"` | Ideas, added to the backlog as `kebbie backlog add` adds them. |
| `"start"` | Starts of a backlog item as a task, as `g` on the board starts one. |
| `"edit"` | Edits of a backlog item's title or one of its sections. |
| `"review"` | A task's waiting step opened for review, line comments typed into its Claude, files marked read, and the step accepted. |

```toml
[hub]
commands = ["answer", "prompt", "idea"]
```

- Unset, the machine takes all seven. `[]` takes nothing, and the board still posts; your phone shows each machine's choice under Machines.
- The single words of earlier versions still read: `"answer-and-push"` is `["answer", "push", "prompt"]`, `"answer"` is `["answer", "prompt"]` and `"off"` is `[]`.
- A refused command gets an answer that names it, such as `Not pushed: devbox takes no pushes from the hub. Push at the machine, or allow pushes in its kebbie settings.`
- A change counts from the next command; `kebbie hub status` shows it on its `commands:` line.

`hub.trust` says which of your devices this machine takes commands from. Every command names the device that sent it, and [`kebbie hub devices`](commands.md#kebbie-hub-devices) lists them.

| Value | A device's first command |
| --- | --- |
| `"device"` | Waits on this machine until you approve the device under `N` on the board or with `kebbie hub devices approve <name>`, or `hub.auto_approve` lets it in. |
| `"account"` | Goes in at once from any device signed in to the account this machine is connected to. |

- A command waits at most its hour. One that expires before the approval is answered `not-approved`, and the device says it was not delivered.
- A device you deny stays view only on this machine, and its commands are answered `denied`.
- A device whose key changed is a new device to approve, and the device it was keeps its own approval. Compare the key under Account on the device with the one kebbie shows.
- `"24h"` for `hub.auto_approve` approves a waiting device 24 hours after its first command unless you deny it first; `"never"` keeps it waiting until you approve it.
- `"devices"` for `hub.approve_from` lets a device you approved on this machine approve or deny a waiting one, from its notification or the phone's board, as well as `N` and `kebbie hub devices`; `"machine"` takes that decision only here. A waiting device never decides on another, and under `hub.commands = []` no device does. See [Approve a new device from another device](../how-to/see-your-board-on-your-phone.md#approve-a-new-device-from-another-device).
- While config.toml does not parse, the machine takes no commands through the hub, lets no device in by time or by account, and takes device decisions only here. The keeper's log, `kebbie hub status` and `kebbie doctor` say so until the file parses again.

### CI, updates and the keeper

| Key | Default | Where | What it does |
| --- | --- | --- | --- |
| `ci.watch` | `true` | either | Whether kebbie watches CI after a push and closes the task only once every workflow run for the pushed commit is green. |
| `comments.watch` | `true` | either | Whether kebbie reads the comments colleagues leave on the commits it pushed and shows each under COMMENTS on the board. |
| `comments.watch_days` | `7` | either | How many days after a push kebbie checks its commits for new comments, a whole number from 1 to 90. |
| `update.channel` | `"release"` | config.toml only | What `kebbie update` installs: `"release"`, the highest `vX.Y.Z` tag on origin, or `"main"`, origin's default branch. |
| `update.agents` | `"ask"` | config.toml only | What happens when Codex, which does not update itself, has a newer version: `"ask"` offers it in a window after `U` on the board, `"auto"` has `U` install it with Codex's own installer, `"notify"` only puts a notice on the board and a line in `kebbie doctor`. kebbie checks once a day. |
| `restart.auto` | `true` | config.toml only | Whether kebbie restarts the Claude sessions and Codex tasks that run older code than is installed, after an update or a new Claude Code or Codex; no other session restarts. |
| `restart.idle_minutes` | `10` | config.toml only | How long, an integer from 1 to 1440 minutes, a session on older code waits to be idle and out of focus before that restart. |
| `keeper.enabled` | `true` | config.toml only | Whether the keeper, one background kebbie per machine, runs the routines. |
| `recover.enabled` | `true` | config.toml only | Whether kebbie resumes a task whose Claude an API error stopped, and puts a Codex task an API error stopped under Needs you. |

`ci.watch` needs origin on GitHub and `gh` logged in. `false` closes the task right after the push, as for a repo without useful CI. See [Watch CI after a push](../explanation/trunk-based.md#ci-after-the-push).

`comments.watch` needs what `ci.watch` needs, origin on GitHub and `gh` logged in, and [`kebbie doctor`](requirements.md#what-kebbie-doctor-checks) says when a repo misses either. Your own comments are skipped, and so are a bot's. kebbie asks `gh` for a repo's new commit comments every 12 minutes while any of its pushed commits is watched, in one call however many are, and less often while `gh` fails. A comment already on the board stays there after the days run out. See [Act on comments on pushed commits](../how-to/act-on-comments-on-pushed-commits.md).

`restart.auto`: each restarted session resumes its conversation, and `kebbie update` restarts the idle ones at once. A session mid-turn, the pane you are in and one with background work running are never restarted on their own. `false` restarts nothing on its own; the board still marks the sessions on older code.

`keeper.enabled`: the keeper's routines then go on with the board closed. `false` runs them in an open board instead, and the board then posts the hub's board once a minute. See [The keeper](../explanation/updates-and-restarts.md#the-keeper). The routines are:

- held messages and delivery checks of typed prompts, stale-session restarts and the update check;
- once an hour, linking kebbie in herdr again when herdr links it from a folder that is gone;
- the push lines and local-main staging, CI watches, comments on pushed commits, closing landed tasks and rebasing the tasks that follow main;
- relaying a task's Claude's own prompts to the lead, resuming tasks an API error stopped, and the rules check on a ready report;
- rechecking kebbie issues after an update, the scheduled `[[routines]]`, and starting the queued tasks whose wait is over;
- posting the board to a [kebbie hub](../how-to/see-your-board-on-your-phone.md), sending error signals and bug reports to kebbie's maintainers, and with `report.inbox_token` fetching the reports inbox.

`recover.enabled`: see [When the API stops a task](../explanation/lead-and-away-mode.md#when-the-api-stops-a-task).

- After overload, a server error, a rate limit, a timeout or a dropped connection, kebbie tells the task to carry on after 1, 2, 4 and 8 minutes. The schedule is fixed.
- After a usage limit, it carries on once the allowance resets.
- Login, billing, invalid request and refusal errors go to you under Needs you at once.

### Reports and issues

| Key | Default | Where | What it does |
| --- | --- | --- | --- |
| `report.signals` | not set (off) | config.toml only | Whether kebbie sends its maintainers error signals when kebbie itself fails, and the daily install signal. `kebbie setup` asks. |
| `report.text` | `"ask"` | config.toml only | What happens to a bug or wish about kebbie, once you or the lead saved it: `"ask"`, `"always"` or `"never"`. |
| `report.enabled` | none | config.toml only | The older single switch, still read: `false` means `signals = false` and `text = "never"`, unless those keys are set. |
| `report.inbox_token` | none | config.toml only | For kebbie's maintainers: the `kha_` token the issues endpoint issued. |
| `issues.propose` | `true` | config.toml only | Whether the lead proposes saving a [kebbie issue](../how-to/report-a-problem-with-kebbie.md) when kebbie itself cost something. |

`report.signals`: see [Error signals](../how-to/report-a-problem-with-kebbie.md#error-signals).

- A panic and some internal errors are counted.
- A signal holds only kebbie's version and commit, the OS and CPU architecture, the component (a fixed name from kebbie's code, such as `keeper.push`), the error kind (`panic`, `internal-error` or `corrupt-state`) and a count. Never an error message, a path, a repo, task or host name.
- The keeper sends at most one batch a day, and each send is a line under messages on the board.
- The install signal goes at most once a UTC day and holds only a random id made for this install, kebbie's version, the OS and CPU architecture and the update channel. See [The install signal](../how-to/report-a-problem-with-kebbie.md#the-install-signal).
- Not set, nothing is counted or sent and no install id is made; `kebbie doctor` says `error signals: not asked`. `kebbie setup` asks, with No preselected, and `kebbie setup --yes` leaves it unset.
- `true` sends both.
- `false` sends neither, drops what was counted and deletes the install id.

`report.text`: see [Send it to kebbie's maintainers](../how-to/report-a-problem-with-kebbie.md#send-it-to-kebbies-maintainers).

- `"ask"`: the board shows the redacted text as it would go to kebbie's maintainers, and only your `y` sends it.
- `"always"`: once you arm it with `kebbie issue report --always` in your own terminal, kebbie sends it, redacted the same way, without asking, and says so under messages. Until then, or after `kebbie issue report --ask`, it asks as with `"ask"`.
- `"never"`: it stays on this machine. `kebbie issue report <id>` still sends one when you ask.

`report.inbox_token`: the keeper then files each report the endpoint lets through, and each error signal at least three installs reported on one day, as a kebbie issue every 10 minutes, each with a line under messages on the board, `New bug report: <title>` (`New wish report: <title>` for a wish), that is for you and not the lead, and `kebbie doctor` counts the reports in quarantine. `kebbie config` shows only whether it is set.

`issues.propose`: the lead proposes one at a calm moment between rounds, and saves it on your yes. `false` keeps the lead from holding, proposing or filing them; it saves one only when you ask it to. It takes effect when the lead next restarts (`L` on the board).

## Backlog sources

| Source | Items |
| --- | --- |
| `"local"` | The local backlog, `backlog/` next to the machine config, one file per item. See [Files](files.md#the-local-backlog). |
| `"vault"` | The vault's backlog notes. Needs the vault plugin. |
| `"linear"` | Your open Linear issues, read by the lead through a Linear MCP server. |

## URL placeholders

`worktree.dev_url` fills these in. A worktree without what the template names gets no link.

| Placeholder | Value for `wt1.app` |
| --- | --- |
| `{worktree}` | `wt1.app`, the worktree's folder |
| `{repo}` | `app` |
| `{number}` | `1`, its pool number |
| `{port}` | the worktree's port: `PORT` in its `.env`, else the port in its setup note |

## Notify command

kebbie runs `notify.command` through `sh -c`, detached, and stops it after 10 seconds. Failures go to `notify.log` in the state directory and never fail the step that sent them. A task sends at most one notification per event a minute, and none while it is [parked](board.md#parked).

The command gets:

| Variable | Value |
| --- | --- |
| `KEBBIE_EVENT` | `needs-you`, `ready`, `question`, `summary`, `ci-failed`, `ci-green`, `api-error`, `routine-findings`, `routine-failed` or `device` |
| `KEBBIE_TASK` | The task's name |
| `KEBBIE_REPO` | The task's repo |
| `KEBBIE_TEXT` | One line, such as `billing@app needs you: answer which queue` |
| `$1` | The same text |

| Event | Sent when |
| --- | --- |
| `needs-you` | A task reports that it needs you, or a routine's Claude asks whether you trust its folder. |
| `ready` | A task reports ready. |
| `question` | A task asks you a question. |
| `summary` | You or the lead run `kebbie notify "<text>"`. Tasks cannot. |
| `ci-failed` | A workflow run for a pushed commit failed. The text names the workflow and the job. |
| `ci-green` | Every workflow run for a pushed commit passed, or none ran. |
| `api-error` | An API error stopped a task and kebbie will not resume it. The text gives the reason and Claude Code's words. |
| `routine-findings` | A routine's task found something: its first report or question, in place of that report's `needs-you`, `ready` or `question`. |
| `routine-failed` | A routine's run could not start, or its task closed before it reported. |
| `device` | A new device on your account sent this machine its first command, which waits for you to approve or deny the device. |

An `api-error` is a login, billing, invalid request or refusal error, or one that persisted through every retry. A routine run that ends with `kebbie routine ok` sends nothing.

## `[[actions]]`

Repo config only. Each table adds an action to the repo's row on the board's Areas page.

| Key | Type | What it does |
| --- | --- | --- |
| `id` | string | The action's name for `then`. Defaults to the title, slugged. |
| `key` | one character | The key that runs it. Not one of `g s b l u a j k q ?`. |
| `title` | string | What the board shows. |
| `about` | string | A line under the title. |
| `ask` | string or list | Values asked in turn, used as `{name}` in the other keys. A value named `version` is prefilled with the next patch version. |
| `bump` | list | Files whose version lines change: a path, or `{ file = "<path>", after = "<marker>" }` to change only the first version line after the marker. `{package}` in a marker is the repo's package name from `pyproject.toml`, `Cargo.toml` or `package.json`. |
| `run` | list of strings | Shell commands run in the main checkout after the bump. |
| `commit` | string | The commit message to create. |
| `tag` | string | The tag to create. |
| `push` | boolean | Push main and the tag as you, after asking once more. |
| `task` | string | Start a kebbie task with this prompt instead of running a script. |
| `then` | list of `"repo:action"` | Actions offered once this one is done. Never run on their own. |

- A script action runs only in a main checkout that is on the default branch, clean and not behind origin.
- The window shows every step first, including the exact lines a bump changes, and nothing changes until you press `y`.
- A bump touches only lines that mention a version and hold the current one. Paths stay inside the repo.

```toml
# ~/.config/kebbie/repos/billing.toml
[[actions]]
key = "R"
title = "Release"
about = "bump, commit, tag, push"
ask = "version"
bump = ["Cargo.toml", { file = "Cargo.lock", after = 'name = "billing"' }]
run = ["cargo build"]
commit = "{version}"
tag = "v{version}"
push = true
then = ["web:bump-client"]

[[actions]]
id = "bump-client"
key = "P"
title = "Bump the client"
ask = ["package", "version"]
task = "Bump {package} to {version}; commit."
```

## `[[routines]]`

Machine config or repo config. Each table is a check kebbie runs on a schedule as a task. See [Routines](../explanation/routines.md) and [Add a routine](../how-to/add-a-routine.md).

| Key | Default | What it does |
| --- | --- | --- |
| `id` | required | The routine's name: lower-case letters, digits and single dashes, starting with a letter, at most 40. |
| `title` | required | What `kebbie routines` and the run's prompt call it. |
| `repo` | the repo config's repo | The repo the run's task starts in. Required in config.toml. |
| `every` | | An interval: `"30m"`, `"6h"`, `"1d"`. A run is due once it has passed since the last run started. |
| `at` | | A local time: `"07:00"` every day, or `"mon 07:00"` once a week (`mon` to `sun`). |
| `prompt` | required | What to check, in your words. |
| `mode` | `"report"` | What the task does with what it finds: `"report"`, `"plan"` or `"fix"`. |
| `push` | `false` | Fix routines only: the run's task carries the push permission the board's `a` gives. |
| `enabled` | `true` | `false` keeps the routine off the schedule; `kebbie routine run <id>` still starts it. |
| `profile` | `task.profile`, else `task` | The [profile](#profiles) the run's task starts with. |

- `id` is unique across config.toml and every repo config. Its runs' tasks are named `<id>-<yyyymmdd-hhmm>`, the id cut to fit herdr's 32 characters.
- `repo` is a checkout's folder name, as `kebbie start --repo` takes it, or its path (`~/code/infra`).
- With `at`, a run is due once that time has come since the last run started. Give `every` or `at`, not both.
- `mode`: `"report"` reports it, `"plan"` proposes a plan for your approval, `"fix"` fixes it in commits.
- With `push`, the lead pushes the fix once its commits hold to the rules.
- A routine's task runs in `auto` mode whatever the profile's `permission_mode` says.
- The board's [Routines page](board.md#routines-page) and `kebbie routine add` write new ones into config.toml, and edit, pause and delete them in the file they are in.
- A routine that is not valid is an error that names its file, its index and the key, as for every other key. `kebbie config` lists the routines it found.

```toml
# ~/.config/kebbie/repos/infra.toml
[[routines]]
id = "cluster-health"
title = "Cluster health"
every = "6h"
prompt = "Check that every node is Ready, no pod is crash-looping and every volume is healthy."
```

## Profiles

A profile names the agent, model, extra role, skills, permission mode and rules a task starts with. Each is a file, `~/.config/kebbie/profiles/<name>.toml`, its name lower-case letters, digits and dashes, starting with a letter. A task gets kebbie's own role, hooks, deny rules and push block whatever its profile says; a profile only adds to them.

Three are built in:

| Profile | What it is |
| --- | --- |
| `task` | kebbie's task as it is: nothing added. Tasks start with it unless `--profile` or `task.profile` names another. |
| `ops` | kebbie's task whose `ssh`, `scp` and `rsync` run outside the sandbox with your ssh agent, for work on other machines; `git push` stays refused. An ssh session reaches this machine as you too, so kebbie says so when the task starts. |
| `reviewer` | The read-only reviewer `kebbie review` runs unless `review.profile` names another review profile. It is never a task's profile. |

A file named `task.toml`, `ops.toml` or `reviewer.toml` adds its keys to that built-in profile.

The board's [Settings page](board.md#profiles) sets a profile's `model` and `effort` from the agent's own list and its `role` from the role files in the folder, writing only that key, and adds and deletes profiles.

| Key | Default | What it does |
| --- | --- | --- |
| `kind` | `"task"` | `"review"` makes it a review profile, which `review.profile` and `review.second` name and no task starts with. |
| `about` | none | One line on what the profile does. `kebbie profiles` shows it, and the task's role opens its profile section with it. |
| `for` | none | The work it fits, in your words. The lead chooses a task's profile by it. |
| `agent` | `"claude"` | The agent CLI the task runs: `"claude"` or `"codex"`. The built-in profiles run Claude. |
| `model` | none | The model, as `claude --model` or `codex --model` takes it. Unset leaves it to the agent's default. |
| `effort` | none | `"low"`, `"medium"`, `"high"`, `"xhigh"` or `"max"`, as `claude --effort` takes it. A Codex profile takes Codex's own levels, `"none"`, `"minimal"`, `"low"`, `"medium"`, `"high"`, `"xhigh"`, `"max"`, `"ultra"` or `"persistent"`, and its task gets it as `model_reasoning_effort`. Which a model takes is Codex's list for it. |
| `role` | none | A markdown file in the profiles folder, carried in the task's role after kebbie's own, under `## Your profile: <name>`. |
| `skills` | `[]` | Skills the task's Claude runs once its worktree is set up, named as `task.rules_skill` is. |
| `permission_mode` | none | The permission mode, one of `task.permission_mode`'s values, ahead of it and behind `kebbie start --mode`. |
| `deny` | `[]` | Claude Code permission rules added after kebbie's own deny rules. |
| `allow` | `[]` | Claude Code permission rules added to the task's allow list. |
| `claude.plugins` | `[]` | Claude Code plugins to turn on for the task, as `<plugin>@<marketplace>`. |
| `sandbox.enabled` | `task.sandbox` | Whether this profile's tasks run sandboxed, ahead of `task.sandbox`. |
| `sandbox.allowed_domains` | kebbie's registries | The hosts a task's shell may reach, in place of kebbie's list. |
| `sandbox.allow_write` | kebbie's caches | The folders a task's shell may write besides its worktree and temp folder, in place of kebbie's list. |
| `sandbox.excluded_commands` | `[]` | Commands that run outside the sandbox, added to kebbie's own and the built-in profile's. One that runs `ssh`, `scp` or `rsync` gives the task your ssh agent. |
| `sandbox.kubeconfig` | none | The one kubeconfig file the task's `kubectl` and `helm` use, from `/` or `~/`. They run outside the sandbox with `KUBECONFIG` set to it. Unset, a sandboxed task reaches no cluster and kebbie refuses `kubectl` and `helm`; with the sandbox off they run with your own logins. |

- `agent = "codex"` runs OpenAI's Codex CLI, which needs kebbie's plugin installed with [`kebbie setup codex`](commands.md#setup-and-maintenance); see [Run a task with Codex](../how-to/run-a-task-with-codex.md).
- `role` is a path relative to the profiles folder: no absolute path, no `..`, and not a symlink.
- `skills`: the role names them; this is an instruction, not a guarantee.
- `permission_mode`: a Codex task gets it as Codex's sandbox and approval flags, where only `"auto"` lets Codex's own reviewer approve a command outside the sandbox; see [How the sandbox and the permission mode map](../how-to/run-a-task-with-codex.md#how-the-sandbox-and-the-permission-mode-map).
- `allow`: a rule that would let the task run `kebbie push`, `approve`, `choose`, `away`, `decide` or `after`, and a rule that allows every shell command, are refused when the profile loads. This check reads the rule's text; what keeps a task from pushing, approving or deciding is kebbie's deny rules, the push block and kebbie's own check of who calls it.
- `claude.plugins` go into the task's settings as `enabledPlugins`. The plugin must be installed.
- `sandbox.allow_write`: each folder from `/` or `~/`. A folder that holds or lies inside kebbie's state, config or install, herdr's config, `~/.claude` or a shell startup file is refused when the profile loads.
- `sandbox.excluded_commands` are in Claude Code's syntax (`docker compose *`). One that names kebbie, herdr, tmux, screen or zellij, or starts with a wildcard, is refused when the profile loads. Such a command reaches everything you can. One that names `kubectl` or `helm` is refused too: set `sandbox.kubeconfig` instead.
- `sandbox.kubeconfig`: the sandboxed shell and Claude's Read and Edit tools still read none of `~/.kube` nor the file. kebbie refuses a `kubectl` or `helm` command that picks another login or cluster, sets `KUBECONFIG`, or names or redirects to a file outside the worktree, with the sandbox off as well. A profile file that is a symlink, such as one linked in from a team repo, may not set it. `kebbie doctor` warns when the file is missing, readable by others or your default `~/.kube/config`. What the task may do in the cluster is the login's RBAC; see [Manage a cluster from a scheduled task](../how-to/manage-a-cluster-from-a-scheduled-task.md).
- `deny`, `allow`, `claude.plugins` and every `sandbox` key but `enabled` and `kubeconfig` are Claude Code's: a profile with `agent = "codex"` that sets one does not load, and the error names the key.

The `reviewer` profile, and every profile with `kind = "review"`, takes `kind`, `about`, `for`, `agent`, `model`, `effort` and `role`; the built-in `reviewer`'s `agent` is only `"claude"`, as the assessor and the rules check run it too. A reviewer only reads, so its tools, permission mode and sandbox are kebbie's. A Claude reviewer's sandbox follows `task.sandbox`, with no host, folder or command outside it; a Codex reviewer runs in Codex's read-only sandbox. Its `role` goes after the review skill. A built-in profile keeps its kind: `kind` in `task.toml` or `reviewer.toml` that says otherwise does not load.

When a profile does not load:

- It is an error that names its file and key, and a task asked to start with it does not start.
- A running task whose recorded profile no longer loads, when it is restarted, goes on as the built-in `task`. kebbie says so with the file and the error: in `kebbie start`'s answer, or to the lead for a restart by `kebbie restart` or the keeper.
- `kebbie profiles` lists the profiles with what each is for, and names a file it ignored with the reason.

```toml
# ~/.config/kebbie/profiles/designer.toml
about = "UI work: a mockup first, measured in the browser"
for = "UI, CSS, new screens, mockups"
model = "opus"
effort = "high"
role = "designer.md"
skills = ["frontend-design:frontend-design"]
permission_mode = "acceptEdits"
allow = ["Bash(npx playwright:*)"]
```

## Example files

```toml
# ~/.config/kebbie/config.toml
[board]
groups = ["needs-you", "ready-to-push"]

[board.backlog]
sources = ["local", "vault"]
ongoing = "bottom"

[worktree]
dev_url = "https://{worktree}.test"

[task]
rules_skill = "my-rules"
also_noticed = "backlog"

[review]
model = "opus"

[notify]
command = 'curl -s -d "$KEBBIE_TEXT" https://ntfy.sh/my-kebbie-topic'

[update]
channel = "release"

[restart]
auto = true
idle_minutes = 10

[keeper]
enabled = true

[recover]
enabled = true
```

```toml
# ~/.config/kebbie/repos/app.toml
[worktree]
setup_command = "script/prepare_worktree"
dev_server = "bin/dev"
dev_worker = "bin/jobs"

[task]
test_commands = ["script/test"]

[ci]
watch = false

[docs]
required = true
```

## Claude Code plugin option

| Option | Default | What it does |
| --- | --- | --- |
| `auto_open` | `false` | Open the review split next to a task's Claude on its first file edit in a session, without taking focus. |

It works only inside herdr and in a git worktree. Closing the pane keeps it closed for that session.

Set it under `/plugin` → Installed → kebbie → Configure, or at install with:

```
claude plugin install kebbie@kebbie --config auto_open=true
```
