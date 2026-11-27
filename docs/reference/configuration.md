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
- In a repo config, `""` unsets `worktree.setup_command`, `worktree.dev_server`, `worktree.dev_worker`, `worktree.release_command`, `review.model` and `ci.deploy` for that repo, so config.toml's value does not apply there, and `kebbie config` shows the key as `unset (repo config)`. In config.toml, `""` for these is an error: leave the key out. Other keys are turned off by a value of their own, such as `false`, `[]`, `"none"` for a profile, or `""` for `worktree.dev_url`.
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
| `board.backlog.sources` | `["local"]` | config.toml only | Where the backlog finds work and new items are written: `"local"` or `"vault"`, and `"linear"` beside either. See [Backlog sources](#backlog-sources). |
| `board.backlog.ongoing` | `"top"` | config.toml only | Where the Backlog page shows ONGOING, the vault notes in progress: `"top"`, `"bottom"` or `"hidden"`. |
| `board.backlog.template` | none | config.toml only | A file with the sections kebbie drafts local backlog items in. See [Backlog sources](#backlog-sources). |
| `board.groups` | the order below | config.toml only | The order of the Work column's groups. |
| `board.next_question` | `false` | config.toml only | Whether the questions window opens the next waiting question after an answer is sent; `false` closes it back to the board with the answer's notice, the cursor on the row you answered from. See [The inbox](board.md#the-inbox). |
| `board.tab_questions` | `false` | config.toml only | Whether `tab` and `shift-tab` move between the waiting questions in the questions window; `false` keeps the window on the question you opened. See [The inbox](board.md#the-inbox). |
| `[keys]` | the board's own | config.toml only | The board's keys by action, a table of strings, as `inbox = "I"`. See [Board](board.md#keys-in-configtoml). |

- `board.backlog.sources`: new items are written to `"local"` or `"vault"`, so a list holds one of them; Linear is only read. A config that lists both reads as the vault alone, and `kebbie doctor` says so. `[]` turns the backlog off.
- `board.backlog.ongoing`: above the backlog items, below them, or not at all. Hidden, a note another machine claims still asks before it starts here.
- `board.backlog.template`: a JSON or TOML file, read from the folder of config.toml when the path is relative. Unset, kebbie drafts a short local item in its own sections: Background, Goal, What we know, Open questions, Out of scope and Source. A short vault item is drafted in the vault's task template, and kebbie's own when the vault has none. The template also names the heading an item's questions sit under; a written item keeps whatever shape its [style](vault-contract.md#the-backlog-style) gave it. `kebbie backlog template --local` shows the one in use.

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
- `board.groups`: a list of `"needs-you"`, `"working"`, `"in-review"`, `"ready-to-push"`, `"going-out"`, `"ci-running"`, `"waiting-for-work"`, `"waiting-on"`, `"queued"`, by default in that order. Groups the list leaves out follow in the default order, so `["ready-to-push"]` moves only Ready to push to the top. A name not in the list is an error that names the valid ones.
- `[keys]`: [Board](board.md#keys-in-configtoml) has the actions and the rules. `kebbie config` shows each key in effect and why an entry was ignored.

### Worktrees

| Key | Default | Where | What it does |
| --- | --- | --- | --- |
| `worktree.setup_skill` | `"kebbie:set-up-worktree"` | either | The skill a task's Claude runs first to make its worktree runnable, or a Markdown file of steps. |
| `worktree.setup_command` | none | either | The repo's own command that prepares a worktree, run from inside it by the setup skill. |
| `worktree.dev_server` | none | either | The command the board's `w` runs in a server tab of the task's Space. Unset, there is no `w`. |
| `worktree.dev_worker` | none | either | A second command, such as a job runner, that `b` runs in a worker tab. Unset, there is no `b`; `""` in a repo config unsets config.toml's for that repo. |
| `worktree.dev_url` | `"http://localhost:{port}"` | either | The running dev server's address, shown on the board as a link. `""` turns the link off. |
| `worktree.dev_route` | `false` | repo config only | Whether [`kebbie routes`](commands.md#setup-and-maintenance) writes a Caddy site for each of this repo's worktrees. |
| `worktree.stage_on_main` | `false` | repo config only | Whether [`kebbie try`](commands.md#kebbie-try-name) (`M` on the board) may put a ready task's commits on this repo's local default branch. |
| `worktree.clean_on_release` | `["target"]` | either | Build output kebbie removes from a worktree when it goes back to the pool. `[]` keeps everything. |
| `worktree.release_command` | none | either | The repo's own command that gives back what a worktree's setup made outside it, such as its own database copy, when the worktree goes back to the pool. |
| `worktree.server_name` | `"server"` | config.toml only | The name of the tab `w` starts, and of the Space's tag in herdr's sidebar while it runs. |
| `worktree.worker_name` | `"worker"` | config.toml only | The same for `b`'s tab. |

- `worktree.setup_skill`: `plugin:skill` for a plugin's skill; a bare name for one in `~/.claude/skills` or the repo's `.claude/skills`; or a Markdown file by an absolute or `~` path, such as `"~/code/app/SETUP.md"`, whose text the task gets in its role.
- `worktree.setup_command`: a command naming a path applies only in a repo that has that file. Where it does not apply, and when unset, the skill runs `bin/setup-worktree` if the repo has one.
- `worktree.dev_server` and `worktree.dev_worker` run with `PORT` set to the worktree's port. A command naming a path, such as `bin/dev`, applies only in a repo that has the file: set in config.toml, it gives no `w` or `b`, and no server or worker line, in a repo without it.
- `worktree.dev_url` fills in the [URL placeholders](#url-placeholders).
- `worktree.dev_route`: each site is at the host `dev_url` names, proxied to the worktree's port. It is opt-in per repo, so a repo that serves its worktrees itself stays out. See [Open dev servers from a remote machine](../how-to/open-dev-servers-from-a-remote-machine.md).
- `worktree.stage_on_main`: the commits stay unpushed, to try them from the main checkout before you push main. It is for a repo you run from its main checkout; kebbie's own repo, when you run kebbie from your clone, turns it on. See [Try a task before pushing](../how-to/try-a-task-before-pushing.md).
- `worktree.clean_on_release` runs on `done`, `drop`, `reset` and a landed task the keeper closes, so the pool's disk use stays bounded. The result says what it freed, as `freed: removed target/ (3.7 GB)`. Its entries are folders inside the worktree, such as `"node_modules/.cache"`, `".next"` or `"dist"`. Each is removed only when git ignores it and tracks nothing in it, a symlink is never followed, and `"target"` only where the worktree has a `Cargo.toml`.
- `worktree.release_command` runs with `sh -c` from inside the worktree once it is back on the default branch, on the same four occasions, before `clean_on_release`, so the repo's own version of the command runs and never one a task changed. In a sandboxed task's worktree, a command whose program is a path git does not track as the default branch has it is not run, and the freed line says so. Its result opens the same line, as `freed: ran dev worktree-db remove; removed target/ (3.7 GB)`. A command that fails, or runs past 2 minutes and is stopped, is named there with its exit and the last line it printed, and the worktree goes back to the pool all the same. A command naming a path applies only in a worktree that has the file, as for `setup_command`.

### Tasks

| Key | Default | Where | What it does |
| --- | --- | --- | --- |
| `task.rules_skill` | `"kebbie:work-rules"` | either | The skill with the rules for how work is planned, built and committed, or a Markdown file of rules. |
| `task.test_commands` | `[]` | either | This repo's test commands: kebbie records them when a task's Claude runs them, on top of the built-in ones, and names them to every agent that runs or asks for tests. |
| `task.also_noticed` | `"backlog"` | either | What a task's Claude does with what it notices outside its task: `"backlog"`, `"fix"` or `"ask"`. |
| `task.profile` | none | either | The [profile](#profiles) a task starts with when `kebbie start` names none with `--profile`. |
| `task.sandbox` | `true` | either | Whether a task's and the reviewer's shell commands run in Claude Code's sandbox. |
| `task.steps` | `true` | either | Whether tasks work in subtasks, each a slice of the work in small commits, listed with `kebbie subtasks` before they write code. |
| `task.require_accept` | `false` | either | Whether every commit must be accepted before it can be pushed. Only with `task.steps` on. |
| `task.permission_mode` | none | either | The [permission mode](https://code.claude.com/docs/en/permission-modes) a task's Claude starts in. |
| `[[routines]]` | none | either | Checks kebbie runs on a schedule, each as a task that reports, plans or fixes. See [`[[routines]]`](#routines). |

`task.rules_skill`: a task's Claude follows it, the reviewer checks a ready task against it, and `tidy` reshapes commits by it. It names a skill as `worktree.setup_skill` does, or a Markdown file by an absolute or `~` path, such as `"~/code/team/RULES.md"`: the task gets the file's text in its role and the reviewer in its brief. With the default, kebbie also refuses a ready report while a commit lacks a subject, a blank line and a body.

`task.test_commands`: the built-in ones are `cargo test`, `bin/rails test`, `pytest`, `npm test`, `go test` and others; `kebbie config` lists them. A command counts when one of these, whole words, starts it or any part of its `;`, `&&` or `||` chain, after `cd`, `export`, `VAR=value` and wrappers such as `timeout` or `nice`, and each one in the chain is recorded as a run of its own. `kebbie doctor` warns when the repo has `script/test`, `bin/test`, `scripts/test`, `script/spec` or `bin/spec` and the list leaves it out. See [`kebbie evidence`](commands.md#kebbie-evidence-name---json). The commands listed here are the ones kebbie names to its agents: a task's role tells its Claude to run its tests through them and start no other test setup, the reviewer's and the investigator's briefs name them for any test run they ask for or make, the assessor's brief names them, and the lead's role tells it to run a task's tests through them.

`task.also_noticed` covers things such as a bug nearby, a stale doc or a refactor. See [Decide what a task does with what it notices](../how-to/decide-what-a-task-does-with-what-it-notices.md).

- `"backlog"` writes it out as a [backlog item](../how-to/fill-the-backlog.md#what-a-written-out-item-holds) and files it where new items go: in the vault's backlog, or with `kebbie backlog add`.
- `"fix"` fixes a small one beside the code it is changing, in a commit of its own, and files the rest.
- `"ask"` puts each to you with `kebbie ask` first.
- Each is listed under Also noticed on the sign-off card. Under `"backlog"` and `"ask"`, the reviewer counts a change outside the task as a finding.

`task.profile`: unset, tasks start with the built-in `task`. `"none"` in a repo's file starts its tasks with the built-in `task` whatever config.toml names. A restarted task keeps the profile it started with. The board's Settings page picks it from the profiles there are, each with its `for`, and `none (task)` removes the key. See [Give tasks a profile](../how-to/give-tasks-a-profile.md).

`task.steps`: on, the default, a task's role tells it to list its subtasks with `kebbie subtasks` before its first line of code and mark each one ready with `kebbie subtasks done`, and you accept them and push the accepted run from the top while it builds on. `false`, in the machine config or in a repo's own file, turns it off: a task commits its own way and `kebbie subtasks` refuses. New tasks follow it; a running task once it restarts. `kebbie accept` works either way. See [Work in subtasks](../how-to/work-in-steps.md).

`task.require_accept`: off, the default, accepting a subtask is up to you and a push of the whole task takes what is committed. `true`, in the machine config or in a repo's own file, holds every push until each commit it takes is accepted: `kebbie push`, `--through`, the board's push and push in order refuse with the first subtask not accepted and how to accept it, and a ready task with a subtask to accept waits under Needs you, `accept “<title>”` under its row. With `task.steps` off it has no effect. See [Require every subtask accepted](../how-to/work-in-steps.md#require-every-subtask-accepted).

`task.sandbox` is strict: no retry outside it, and no start without it. See [The trust boundary](../explanation/trust-boundary.md#the-sandbox).

- A task's shell writes only in its worktree, its temp folder, the package caches (`~/.cargo/registry`, `~/.cargo/git`, `~/.npm`, `~/.cache`) and the repo's shared `.git` folder, except its hooks, config and the other pooled worktrees' git dirs.
- It reaches only crates.io, static.crates.io, index.crates.io, registry.npmjs.org, github.com, objects.githubusercontent.com, pypi.org and files.pythonhosted.org, and no Unix socket.
- On macOS it may listen on local ports for a dev server.
- kebbie's own task commands (`peers`, `tell`, `ask`, `report`, `subtasks`, `order`, `routine ok`, `link`, `backlog add`, `backlog --search`, `config`) run outside it.
- On Linux it needs bubblewrap, socat and the seccomp filter (`npm install -g @anthropic-ai/sandbox-runtime`). Without them kebbie refuses to start a task or a review and names what to install.
- `false` runs them unsandboxed, where a task's shell can pass as you to kebbie's user-only commands. `kebbie doctor`, a task's start and the board's Areas page say so.
- A profile's `[sandbox] enabled` wins over it.

`task.permission_mode` is one of `"default"`, `"acceptEdits"`, `"plan"`, `"auto"`, `"dontAsk"` or `"bypassPermissions"`. See [Let tasks run without permission prompts](../how-to/let-tasks-run-without-prompts.md).

- Unset, Claude Code's own default applies.
- `kebbie start --mode` and then the task's profile's `permission_mode` override it, and a restarted task keeps the mode it started in.
- Any other value is refused when the config loads.
- `"bypassPermissions"` runs every tool without asking; Claude Code recommends it only for an isolated container or VM. A task still never pushes nor approves its own commits in it, since kebbie's deny rules and the worktree's push block hold in every mode.

### Push

| Key | Default | Where | What it does |
| --- | --- | --- | --- |
| `push.branch` | none | repo | The branch on origin each task pushes to instead of the default branch, such as `"feature/{task}"`. |

`push.branch`: `{task}` is filled in with the task's name, so `"feature/{task}"` pushes the task `login-sheet` to `feature/login-sheet`. The template must name `{task}`, so every task gets a branch of its own, and kebbie refuses one that makes no branch name git takes. See [Push to a branch per task](../how-to/push-to-a-branch-per-task.md).

- A push goes out as the task's branch holds it, not rebased onto the default branch, and adds to the branch on origin only by fast-forward. kebbie never force-pushes it: a branch whose pushed commits were rewritten is refused until its Claude puts the new commits on top.
- The push line, the rules check, the review and the test runs a push needs hold as without it, counted from where the task left the default branch.
- kebbie watches no CI for it and closes nothing after it: the task waits under [On branch, waiting for main](board.md#on-branch-waiting-for-main) until its commits are on the default branch, and the keeper closes it then.
- Unset, tasks rebase onto the default branch and push to it.
- Repo config only: config.toml refuses it.

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

- `"suggest"`: the lead hears `review suggested: <why>`, and nothing starts until you send it, or the lead on your word. Review is optional: the task stays under Ready to push either way.
- `"run"`: the reviewer starts by itself; the push is not held.
- `"require"`: the push is held from the ready report on, until the reviewer passes.

An assessment with no answer reads as `review not assessed`, a suggestion in every mode. A task you withdraw from review is not assessed again until you send it. See [An assessor judges whether the work needs a review](../explanation/review-gate.md#an-assessor-judges-whether-the-work-needs-a-review).

### Investigation

| Key | Default | Where | What it does |
| --- | --- | --- | --- |
| `investigate.profile` | `"investigator"` | either | The profile kebbie's investigator runs with on an obstacle, a review's `confirm:`, `scope:` or `gap:` finding, or a thing two tasks found; `"none"` runs none. |

`investigate.profile` names any profile in `~/.config/kebbie/profiles/` or a built-in one, and the investigator takes its agent, model, effort and role: a Claude profile with its model and effort, or a Codex one. The built-in `investigator` is Claude at high effort, with your own settings, plugins and MCP servers. With `"none"`, a raised obstacle goes to the lead, which decides it with [`kebbie obstacles decide`](commands.md#kebbie-obstacles-name---json). See [Obstacles and findings](../explanation/obstacles-and-findings.md#one-investigation-per-obstacle).

```toml
[investigate]
profile = "deep-codex"
```

### Writer

| Key | Default | Where | What it does |
| --- | --- | --- | --- |
| `writer.profile` | `"writer"` | config.toml only | The profile kebbie's writer runs with when the lead is asked to write out a backlog item; `"none"` has the lead write it in its own chat. |

An idea from your phone, a short item from `kebbie backlog add` or `n`, and **Write it out** go to the writer first. It reads the code, the vault and the web, with the vault's reading tools, git's reading commands and `kebbie backlog`, `kebbie backlog template` and `kebbie areas` alone, and writes the item out apart from the lead's chat; it saves nothing itself, so the lead's context stays clean. The lead then reviews the draft and its open questions and saves it. `writer.profile` names any profile in `~/.config/kebbie/profiles/` or a built-in one, and the writer takes its agent, model and effort. The built-in `writer` is Claude at medium effort. A writer that cannot start or gives no draft leaves the item to the lead.

```toml
[writer]
profile = "quick-codex"
```

### Machine

| Key | Default | Where | What it does |
| --- | --- | --- | --- |
| `machine.name` | the host's short name | config.toml only | The machine's one name, on your phone through the hub and on the vault notes it claims. |
| `notify.command` | none | config.toml only | The shell command kebbie runs when something needs you. Unset sends nothing. See [Notify command](#notify-command). |
| `lead.profile` | none | config.toml only | The Claude [profile](#profiles) whose model and effort the lead runs with. Unset, the lead runs on the model your Claude Code settings name. |
| `hub.ideas_machine` | none | config.toml only | The `machine.name` of the machine that writes out new backlog items, from your phone and from your other machines. See below. |
| `vault.command` | none | config.toml only | The vault command kebbie runs for task notes, log lines and the board's snapshot, before its own arguments, such as `"uvx mindin"`. |
| `hub.url` | the kebbie hub, `https://hub.kebbie.dev` | config.toml only | The [kebbie hub](../how-to/see-your-board-on-your-phone.md) `kebbie hub connect` and the board's `H` window connect this machine to. |
| `hub.commands` | all eight kinds | config.toml only | What this machine takes from your devices through the hub: a list of `"answer"`, `"push"`, `"prompt"`, `"idea"`, `"start"`, `"edit"`, `"review"` and `"lead"`. See [Answer and push from your phone](../how-to/see-your-board-on-your-phone.md#answer-and-push-from-your-phone). |
| `hub.trust` | `"device"` | config.toml only | Which of your devices this machine takes commands from: `"device"`, each device you approve here, or `"account"`, any device signed in to your account. |
| `hub.auto_approve` | `"24h"` | config.toml only | When a device waiting under `hub.trust = "device"` is let in: `"24h"` after its first command unless you deny it, or `"never"`, only when you approve it. |
| `hub.approve_from` | `"devices"` | config.toml only | Where a waiting device can be approved: `"devices"`, also from a device you approved on this machine, or `"machine"`, only here. |

`machine.name`: your phone shows it through the [hub](../how-to/see-your-board-on-your-phone.md#connect-the-machine), and kebbie claims vault notes under it. See [Two machines, one vault](../explanation/vault.md#two-machines-one-vault).

- `kebbie hub connect`, the board's `H` window and `kebbie hub join` use it, set it from `--name`, or ask for it when it is unset. [`kebbie:configure`](skills.md) asks for it first when it sets kebbie up.
- A change renames the machine on the hub with the keeper's next post, or at once with `kebbie hub status`. While that fails, `kebbie hub status` and `kebbie doctor` say why.
- At most 64 characters, without control characters. `"localhost"` is refused, and `kebbie doctor` warns while the key is unset.
- A task started from a vault note, or one kebbie opens a note for, writes this name and the task's into the note's `kebbie` frontmatter key. kebbie on another machine sharing the vault shows the note as `working on <name> · <task>` and asks before starting it.
- Give each machine its own stable name, such as `"laptop"` and `"workstation"`.

`hub.ideas_machine`: one machine writes out the backlog items you add, wherever you add them.

- The machine it names says so on its board, and the phone's **New idea** picks it while it is awake, and an awake machine while it sleeps. The line under the machine says why: `ideas machine`, or `ideas machine is asleep, sent to workstation`.
- An item you add on another machine with `kebbie backlog add` or `n` goes to the same machine when the backlog is the vault. When its repo has a checkout on only one machine, it goes to that machine instead. The machine you added it on says where it went, such as `Sent to workstation, the ideas machine`, and the item shows on that machine's board. When that machine has not taken it within 5 minutes, as while it sleeps, the machine you added it on takes it back and adds it there.
- Machines find each other through the vault: each one keeps its name, its repos and whether it is the ideas machine in the vault note `Kebbie/machines.md`, and items for a machine wait in its `Kebbie/inbox-<name>.md` until its keeper files them. A machine silent for a week drops out of the note.
- An item you asked to start once it is written goes along and starts on that machine. A task already running stays on the machine where it started, and so does a local backlog without the vault.
- Set it on the machine it names, or the same value on every machine.

```toml
[hub]
ideas_machine = "workstation"
```

`lead.profile`: the lead runs another model than your other Claude sessions. See [Run the lead on another model](../how-to/give-tasks-a-profile.md#run-the-lead-on-another-model).

- It takes the profile's `model`, `effort` and `advisor` alone. Its role, skills and rules are a task's, and the lead keeps its own.
- It names a Claude profile. A Codex profile or a review profile is refused, and so is one that does not load: the lead then starts on Claude Code's own model, and `kebbie doctor` warns.
- It takes effect when the lead starts or restarts: `L` on the board, `kebbie lead --restart`, or the restart after an update.
- The Settings page picks it under This machine from the Claude profiles that set a model or an effort. `none` removes the key.

`vault.command` is kebbie's own way to the vault, whatever agent runs the task: Claude Code, Codex or another.

A `vault.command` that pins an older version of the package the enabled Claude vault plugin pins, as `uvx mindin==0.16.0` beside a plugin on 0.19.0, gives way to the plugin's command: `kebbie update` moves the plugin, and an older client than the vault's server expects loses what the server added. `kebbie doctor` shows which command runs.

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
| `"start"` | Starts of a backlog item as a task, as `g` on the board starts one, a new task from the app's **New**, as `n` starts one, and what tasks found started, added to a task or kept in the backlog. |
| `"edit"` | Edits of a backlog item's title or one of its sections. |
| `"review"` | A task's waiting subtask opened for review, line comments typed into its Claude, files marked read, the subtask accepted, and a task or subtask sent to a full review. |
| `"lead"` | Your messages to the lead, typed into its chat as your own word once it is free, and away mode switched on or off. |

```toml
[hub]
commands = ["answer", "prompt", "idea"]
```

- Unset, the machine takes all eight. `[]` takes nothing, and the board still posts; your phone shows each machine's choice under Machines.
- The single words of earlier versions still read: `"answer-and-push"` is `["answer", "push", "prompt"]`, `"answer"` is `["answer", "prompt"]` and `"off"` is `[]`.
- A refused command gets an answer that names it, such as `Not pushed: workstation takes no pushes from the hub. Push at the machine, or allow pushes in its kebbie settings.`
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
| `ci.deploy` | none | either | The name of the workflow that deploys the repo: a stage whose requirement is `deploy` is met once a run of it on the stage's pushed commit is green. None takes CI green on the pushed commit as the deploy. |
| `ci.deploy_window` | none | repo only | When the repo's deploys may run, such as `"mon-fri 08:00-17:00"`, in this machine's time zone, or in UTC with `UTC` after it. A run GitHub cancels while the window is shut keeps the task under CI running, and in the window's next opening kebbie re-runs it and closes the task once it is green. A run cancelled while the window stayed open goes to Needs you, as without the key. Without the key, a cancelled deploy goes to Needs you as `CI cancelled`, and the lead's notice and `kebbie doctor` suggest the key. |
| `tests.per_commit` | `true` in a repo with a worktree setup, else `false` | either | Whether the keeper tests each of a ready task's unpushed commits on its own and marks the push order with the results. |
| `tests.before_push` | `"related"` | either | Which tests a ready report and a push wait on: `"related"`, the tests the task runs for its change, or `"full"`, a green run of `tests.full_command` on the task's HEAD. |
| `tests.full_command` | none | repo only | The command that runs the repo's whole test suite, such as `"cargo test --locked"` or `"uv run pytest -q"`. |
| `tests.lint_command` | none | repo only | The command that lints the repo as CI does, such as `"cargo clippy --locked --all-targets -- -D warnings"`. A ready report and every push wait for a green run of exactly this command on the task's HEAD, whatever `tests.before_push` says. |
| `comments.watch` | `true` | either | Whether kebbie reads the comments colleagues leave on the commits it pushed, on your own commits and wherever they mention you, and shows each under COMMENTS on the board. |
| `comments.watch_days` | `7` | either | How many days after a push kebbie checks its commits for new comments, a whole number from 1 to 90. |
| `update.channel` | `"release"` | config.toml only | What `kebbie update` installs: `"release"`, the highest `vX.Y.Z` tag on origin, or `"main"`, origin's default branch. |
| `update.agents` | `"ask"` | config.toml only | What happens when Codex, which does not update itself, has a newer version: `"ask"` offers it in a window after `U` on the board, `"auto"` has `U` install it with Codex's own installer, `"notify"` only puts a notice on the board and a line in `kebbie doctor`. kebbie checks once a day. |
| `restart.auto` | `true` | config.toml only | Whether kebbie restarts the Claude sessions and Codex tasks that run older code than is installed, after an update or a new Claude Code or Codex; no other session restarts. |
| `restart.idle_minutes` | `10` | config.toml only | How long, an integer from 1 to 1440 minutes, a session on older code waits to be idle and out of focus before that restart. |
| `restart.lead_context` | `75` | config.toml only | How full the lead's context may get, a whole percentage of its model's window from 10 to 95, before kebbie has it write a handover and starts it fresh to read it. The board's `L` line shows from this share on. |
| `keeper.enabled` | `true` | config.toml only | Whether the keeper, one background kebbie per machine, runs the routines. |
| `worker.enabled` | `false` | config.toml only | Whether this machine runs [jobs](#jobs) and [`kebbie run`](commands.md#commands-on-another-machine) commands for the other machines of its hub account. |
| `link.networks` | `["tailscale", "wireguard"]` | config.toml only | The private networks [`kebbie run`](commands.md#commands-on-another-machine) links over, in the order to try them: `"tailscale"` and `"wireguard"`. |
| `recover.enabled` | `true` | config.toml only | Whether kebbie resumes a task whose Claude an API error stopped, and puts a Codex task an API error stopped under Needs you. |
| `builds.slots` | `1` | config.toml only | How many task builds run at once on this machine, a whole number from 1 to 64. See [Builds](#builds). |
| `builds.commands` | `["cargo"]` | config.toml only | The commands that take a build slot: a program, and the words its arguments start with, such as `"npm run build"`. |
| `disk.trim_below` | `"30 GB"` | config.toml only | Free space, as `"30 GB"` or `"15%"` of the disk, below which the keeper trims build output from worktrees nobody uses. See [Disk space](#disk-space). |
| `disk.builds_below` | `"10 GB"` | config.toml only | Free space below which a task's build does not start. Below `disk.trim_below` when both use the same unit. See [Disk space](#disk-space). |

`ci.watch` needs origin on GitHub and `gh` logged in. `false` closes the task right after the push, as for a repo without useful CI. See [Watch CI after a push](../explanation/trunk-based.md#ci-after-the-push).

`tests.per_commit`: when a task reports ready, the keeper runs the test commands the task's own recorded runs used, else `task.test_commands`, on each unpushed commit, one at a time, in a throwaway worktree of kebbie's, and keeps each result by the commit's sha. Nothing waits on it. The keeper first prepares that worktree with the repo's own setup, `worktree.setup_command` or else `bin/setup-worktree`, and when the setup fails the commits show as not tested. Left unset, it runs only in a repo that has such a setup, since a worktree nobody set up fails every commit for want of its dependencies; `true` runs it without one. A commit the task's own run already tested is not run again, and a task in the sandbox gets only those, since kebbie runs none of its code outside the sandbox. The cost is a suite run per commit and a build in the throwaway worktree, so a repo with a slow suite sets it to `false`. At rest nothing runs. See [Test each commit on its own](../how-to/work-in-steps.md#test-each-commit-on-its-own).

`tests.before_push = "full"` refuses a task's ready report and every push of it, from the board, the lead or your phone, until kebbie recorded a green run of `tests.full_command` on the task's HEAD; a push of only its first commits needs that green run on the commit the push ends at. The refusal names the command to run, and the task's role tells its Claude to run it after its last commit. A run counts when the line runs the command with flags of its own at most (`cargo test --locked -q` counts, `cargo test --locked billing` does not), on the same tree as HEAD. When a push rebases the task onto a moved default branch, its Claude runs the full suite again and kebbie pushes once it reports ready. A repo without `tests.full_command` stays on related tests, and [`kebbie doctor`](requirements.md#what-kebbie-doctor-checks) says so; it also suggests `"full"` with the command for a repo whose recorded whole-suite runs take five minutes or less.

`tests.lint_command` refuses a task's ready report and every push of it in the same way until kebbie recorded a green run of the lint on the task's HEAD, on related tests as on `"full"`. A run counts only when the line runs the command word for word, after `cd`, assignments and redirections at most: `cargo clippy --locked --all-targets -- -D warnings -A clippy::all` is another lint. kebbie records the lint's runs beside the test runs, but a lint run never counts as a test run: a green lint does not hide a red test run, and a red lint is not a red test to excuse with `--red-cause`. A lint of commands joined by `&&`, such as `cargo fmt --check && cargo clippy -- -D warnings`, is owed command by command: each needs a green run on HEAD, and running the whole line word for word gives each its run. A lint joined any other way, by `;` or a pipe, is matched as one command, which no recorded run can be, so join the commands with `&&`. The lint let-off for an error from the default branch needs a green lint run on a commit of the default branch since; a run whose result kebbie could not read shows nothing. When kebbie cannot read the repo's config, its own state, the commit to check, the review's verdict or where the branch left the default branch, it refuses the ready report and the push and says what it could not read. When a push rebases the task onto a moved default branch, its Claude runs the lint again, with the full suite when that is owed too, and kebbie pushes once it reports ready.

A lint red on code the task's branch brought from the default branch, which that branch has fixed since, holds neither the ready report nor the push of the whole task. kebbie takes it once the lint's latest runs are red on HEAD, red on the base the branch left the default branch at, and green on a commit of the default branch since, each matched by its tree, so a later push to main keeps it. The refusal tells the task to show it with `git restore --source=<base> --staged --worktree :/`, the lint, the same with `--source=origin/main`, and `git restore --source=HEAD --staged --worktree :/`, each a command of its own and on its own branch, since a worktree checked out at another commit is free to the pool. A green run on the base, or a red one on main since, makes the error the task's own. The push rebases the task, its base is then the tip, and the lint is owed on the rebased code as above. A push of only the first commits gets no such let-off.

```toml
# ~/.config/kebbie/repos/app.toml
[tests]
before_push = "full"
full_command = "cargo test --locked"
lint_command = "cargo clippy --locked --all-targets -- -D warnings"
```

`comments.watch` needs what `ci.watch` needs, origin on GitHub and `gh` logged in, and [`kebbie doctor`](requirements.md#what-kebbie-doctor-checks) says when a repo misses either. Your own comments are skipped, and so are a bot's. kebbie asks `gh` for a repo's new commit comments every 12 minutes while any of its pushed commits is watched, in one call however many are, and less often while `gh` fails. In a repo with a kebbie worktree pool it asks every 12 minutes regardless, with two more calls for the pull request and issue comments that mention you. A comment already on the board stays there after the days run out. See [Act on comments on pushed commits](../how-to/act-on-comments-on-pushed-commits.md).

`restart.auto`: each restarted session resumes its conversation, and `kebbie update` restarts the idle ones at once. A session mid-turn, the pane you are in and one with background work running are never restarted on their own. `false` restarts nothing on its own; the board still marks the sessions on older code.

`worker.enabled`: the keeper tells the hub this machine is a worker, takes the jobs the other machines on your account's signed list send, and runs them one at a time, in your user account, each in a folder of its task's own under `~/.local/share/kebbie/worker`. Turning it on is your yes to every one of those machines. Jobs come through the hub; for [`kebbie run`](commands.md#commands-on-another-machine) the keeper also listens on port 47011, on loopback and this machine's Tailscale and WireGuard addresses only, and tells the account's other machines those addresses through the hub, and it keeps a WebSocket to the hub open to be rung for a relayed link. Turning it off, or stopping the keeper, tells the hub it is no worker any more and stops the listening. See [Run a job on another machine](../how-to/run-a-job-on-another-machine.md) and [Run a command on another machine](../how-to/run-a-command-on-another-machine.md).

`link.networks`: on a worker, the keeper listens on and announces only its addresses on the networks listed. On the machine that runs `kebbie run`, it dials the listed networks in this order, each next one 150 ms after the one before unless that one failed first, and never an address on a network the list leaves out. `[]` links only through the hub's relay. Any other word is refused. A laptop with both Tailscale and the WireGuard app, whose workstation reaches it only over WireGuard, can keep both, and the workstation sets `["wireguard"]`:

```toml
[link]
networks = ["wireguard"]
```

`keeper.enabled`: the keeper's routines then go on with the board closed. `false` runs them in an open board instead, and the board then posts the hub's board once a minute. See [The keeper](../explanation/updates-and-restarts.md#the-keeper). The routines are:

- held messages and delivery checks of typed prompts, stale-session restarts and the update check;
- handing a freed build slot to the next task in line;
- trimming build output from worktrees nobody uses when the disk runs low ([Disk space](#disk-space));
- once an hour, linking kebbie in herdr again when herdr links it from a folder that is gone;
- the push lines and local-main staging, CI watches, comments on pushed commits, closing landed tasks and rebasing the tasks that follow main;
- relaying a task's Claude's own prompts to the lead, resuming tasks an API error stopped, and the rules check on a ready report;
- rechecking kebbie issues after an update, the scheduled `[[routines]]`, and starting the queued tasks whose wait is over;
- posting the board to a [kebbie hub](../how-to/see-your-board-on-your-phone.md), sending error signals and bug reports to kebbie's maintainers, and with `report.inbox_token` fetching the reports inbox.

#### Builds

Each Claude task runs its builds through a guard kebbie puts first on the PATH of its commands. A build takes one of the machine's `builds.slots`:

- A task that finds every slot taken is told its place in line on a line that starts with `error: kebbie:`, so a filter on error lines keeps it, and its command exits with status 75. The board shows `waiting for a build slot, 2nd` on its row.
- When a slot frees, the keeper tells the next task in line that its turn has come and holds the slot 3 minutes for it. A task that does not build in that time loses its turn to the next, and is told so.
- `kebbie update` goes first in line and waits there, at most 10 minutes, then builds without a slot.
- The head of a repo's [push line](../explanation/trunk-based.md#one-push-line-per-repo) goes next, before every other task, since its push waits on its build. It is told that place as it joins the queue.
- A build that has held its slot 30 minutes while other tasks wait loses it to the next in line and is told so; it runs on outside the queue.
- A command a build runs, such as `cargo` from a build script, runs in its build's slot. The next command of the same script finds the slot it just left for 10 seconds.
- The guard runs cargo with `sccache` as `RUSTC_WRAPPER` when `sccache` is on PATH and neither `RUSTC_WRAPPER` nor a cargo config sets a wrapper, so the machine compiles each dependency once; each worktree builds its own crate in its own `target/`.
- Without a keeper running, a build that finds no slot free for it goes ahead without waiting. Turns that run out and slots of builds that died are freed all the same, by the next build that asks.

`"cargo"` in `builds.commands` counts only cargo's compiling commands: `build`, `check`, `test`, `clippy`, `bench`, `doc`, `install`, `rustc`, `rustdoc`, `fix`, `nextest` and `llvm-cov`. `cargo run` takes no slot, as what it runs, such as a dev server, may never end. A task started after a change to the list has the new one.

```toml
# ~/.config/kebbie/config.toml
[builds]
slots = 2
commands = ["cargo", "xcodebuild", "npm run build"]
```

#### Disk space

Each worktree builds into its own `target/`, a few GB each, so a machine with many tasks can fill its disk. kebbie watches the free space on the disks that hold the repos' worktrees and sccache's build cache (`SCCACHE_DIR`, or `~/.cache/sccache`). The cache itself is never trimmed: when its disk is low, the keeper trims the worktrees on that disk, and the line under Needs you names the disk that stays short.

On Linux, every 2 minutes, when the free space is below `disk.trim_below`, the keeper removes the build output that `worktree.clean_on_release` names from worktrees nobody uses, in this order:

1. Free worktrees in a repo's pool.
2. Tasks waiting for a push: ready, staged, pushing, or with CI running.
3. Tasks whose Claude has finished: waiting for you to read its report, to check its work, or with nothing to push, and tasks whose CI went red.

It skips a worktree when any of these holds:

- its task's Claude is working, or herdr cannot say;
- its task waits mid-work: blocked on an obstacle, the lead or another task, on an answer to its question, kept open after its push, or stopped or failed;
- its task holds a build slot, has a turn, or waits in line for one;
- a process works inside it other than its Claude or Codex, their shells, kebbie, and the tools the agent starts itself such as its MCP servers, so a build or a dev server started through a shell or in the task's pane keeps it; or a program built in it runs;
- for a free worktree, any process works inside it.

It checks each worktree again just before it trims it, and stops once the disk is above the limit. A task whose output was trimmed builds it again the next time it builds. When the free space is still below the limit after a round, the board shows a line under Needs you with the free space. The line goes after the next round that finds enough room, or 10 minutes after the keeper stops. With the keeper off, an open board runs the same round every minute. On macOS kebbie cannot see which processes use a worktree, so it trims nothing there: the line under Needs you shows the free space and says to free it by hand.

When the free space under a worktree is below `disk.builds_below`, a task's build there takes no build slot and does not run. The guard says how much is free, on a line that starts with `error: kebbie:`, and exits with status 75, and a task in line whose own worktree is on such a disk gets no build turn: it keeps its place while the turn goes to the next task whose disk has room.

Each limit is free space in GB, as `"30 GB"`, or a share of the whole disk, as `"15%"`.

```toml
# ~/.config/kebbie/config.toml
[disk]
trim_below = "15%"
builds_below = "10 GB"
```

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
| `issues.propose` | `true` | config.toml only | Whether the lead saves a [kebbie issue](../how-to/report-a-problem-with-kebbie.md) by itself when kebbie itself cost something. |

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

`issues.propose`: the lead saves one by itself, with no question to you, and it waits in the board's bugs to send. `false` keeps the lead from filing them; it saves one only when you ask it to. It takes effect when the lead next restarts (`L` on the board).

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

## `[jobs]`

A repo's jobs, in its repo config on the machine with the tasks: commands a machine with [`worker.enabled`](#ci-updates-and-the-keeper) runs on a worktree for [`kebbie job run <name>`](commands.md#jobs-on-another-machine).

```toml
# ~/.config/kebbie/repos/app.toml
[jobs]
lint = "swiftlint"

[jobs.build]
run = "xcodebuild -scheme App -destination 'generic/platform=iOS Simulator' build"
on = "macos"
timeout_minutes = 90
```

| Form | What it is |
|---|---|
| `name = "<command>"` | A job any worker of your hub account runs. |
| `[jobs.<name>]` with `run`, `on` and `timeout_minutes` | `run` is the command. `on` picks the worker: an operating system, `macos` or `linux`, or a machine's name. `timeout_minutes`, a whole number from 1 to 1440, is how long the job may run, 60 unless set. |

The command runs with `sh -c` in the worktree's files on the worker, its stderr in its output. Past its timeout the worker stops it, SIGTERM to its process group and SIGKILL 10 s later, and once the command ends, whatever it left running is stopped too. Repo config only.

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
- With `push`, kebbie pushes the fix once its commits hold to the rules.
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
| `advisor` | `["fable", "opus"]`, unless Claude Code's own `advisorModel` is set | The advisor model Claude Code consults: a model, a list to try in order, or `"off"`. kebbie asks Claude Code with one short haiku turn whether the account can use each, and writes the first it can as `advisorModel` in the session's settings; none leaves the advisor off. Claude Code itself drops an advisor weaker than the session's model. Each answer is kept in `advisor.json` in the state folder, a week when it worked and a day when it was refused; a check that fails for another reason, such as no network, is kept for nothing. Without the key and with `advisorModel` set in `~/.claude/settings.json`, that setting holds. A Codex profile does not take it. |
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
lead_context = 75

[keeper]
enabled = true

[recover]
enabled = true

[lead]
profile = "deep"
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
