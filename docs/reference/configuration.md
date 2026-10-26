# Configuration

Every key kebbie reads, the file it goes in and what it does. Every key has a default, so kebbie runs without a config file. `kebbie config` prints the keys with the value in effect, the layer it comes from and what it does.

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
| `board.backlog.sources` | `["local"]` | config.toml only | Where the Backlog column finds work: a list of `"local"`, `"vault"`, `"linear"`. See [Backlog sources](#backlog-sources). |
| `board.backlog.ongoing` | `"top"` | config.toml only | Where the Backlog column shows ONGOING, the vault notes in progress: `"top"`, `"bottom"` or `"hidden"`. |
| `board.groups` | the order below | config.toml only | The order of the Work column's groups. |
| `[keys]` | the board's own | config.toml only | The board's keys by action, a table of strings, as `inbox = "I"`. See [Board](board.md#keys-in-configtoml). |

- `board.backlog.sources`: the first source is where the lead files new ideas. `[]` turns the backlog off.
- `board.backlog.ongoing`: above the backlog items, below them, or not at all. Hidden, a note another machine claims still asks before it starts here.
- `board.groups`: a list of `"needs-you"`, `"working"`, `"to-review"`, `"ready-to-push"`, `"ci-running"`, `"waiting-for-work"`, `"queued"`, by default in that order. Groups the list leaves out follow in the default order, so `["ready-to-push"]` moves only Ready to push to the top. A name not in the list is an error that names the valid ones.
- `[keys]`: [Board](board.md#keys-in-configtoml) has the actions and the rules. `kebbie config` shows each key in effect and why an entry was ignored.

### Worktrees

| Key | Default | Where | What it does |
| --- | --- | --- | --- |
| `worktree.setup_skill` | `"kebbie:set-up-worktree"` | either | The skill a task's Claude runs first to make its worktree runnable. |
| `worktree.setup_command` | none | either | The repo's own command that prepares a worktree, run from inside it by the setup skill. |
| `worktree.dev_server` | none | either | The command the board's `w` runs in a server tab of the task's Space. Unset, there is no `w`. |
| `worktree.dev_worker` | none | either | A second command, such as a job runner, that `b` runs in a worker tab. Unset, there is no `b`. |
| `worktree.dev_url` | `"http://localhost:{port}"` | either | The running dev server's address, shown on the board as a link. `""` turns the link off. |
| `worktree.dev_route` | `false` | repo config only | Whether [`kebbie routes`](commands.md#setup-and-maintenance) writes a Caddy site for each of this repo's worktrees. |
| `worktree.stage_on_main` | `false` | repo config only | Whether [`kebbie stage`](commands.md#kebbie-stage-name) (`M` on the board) may put a ready task's commits on this repo's local default branch. |
| `worktree.clean_on_release` | `["target"]` | either | Build output kebbie removes from a worktree when it goes back to the pool. `[]` keeps everything. |
| `worktree.server_name` | `"server"` | config.toml only | The name of the tab `w` starts, and of the Space's tag in herdr's sidebar while it runs. |
| `worktree.worker_name` | `"worker"` | config.toml only | The same for `b`'s tab. |

- `worktree.setup_skill`: `plugin:skill` for a plugin's skill; a bare name for one in `~/.claude/skills` or the repo's `.claude/skills`.
- `worktree.setup_command`: a command naming a path applies only in a repo that has that file. Where it does not apply, and when unset, the skill runs `bin/setup-worktree` if the repo has one.
- `worktree.dev_server` and `worktree.dev_worker` run with `PORT` set to the worktree's port.
- `worktree.dev_url` fills in the [URL placeholders](#url-placeholders).
- `worktree.dev_route`: each site is at the host `dev_url` names, proxied to the worktree's port. It is opt-in per repo, so a repo that serves its worktrees itself stays out. See [Open dev servers from a remote machine](../how-to/open-dev-servers-from-a-remote-machine.md).
- `worktree.stage_on_main`: the commits stay unpushed, to try them from the main checkout before you push main. It is for a repo you run from its main checkout; kebbie's own repo, when you run kebbie from your clone, turns it on. See [Try a task before pushing](../how-to/try-a-task-before-pushing.md).
- `worktree.clean_on_release` runs on `done`, `drop`, `reset` and a landed task the keeper closes, so the pool's disk use stays bounded. The result says what it freed, as `freed: removed target/ (3.7 GB)`. Its entries are folders inside the worktree, such as `"node_modules/.cache"`, `".next"` or `"dist"`. Each is removed only when git ignores it and tracks nothing in it, a symlink is never followed, and `"target"` only where the worktree has a `Cargo.toml`.

### Tasks

| Key | Default | Where | What it does |
| --- | --- | --- | --- |
| `task.rules_skill` | `"kebbie:work-rules"` | either | The skill with the rules for how work is planned, built and committed. |
| `task.test_commands` | `[]` | either | Test commands kebbie records when a task's Claude runs them, on top of the built-in ones. |
| `task.also_noticed` | `"backlog"` | either | What a task's Claude does with what it notices outside its task: `"backlog"`, `"fix"` or `"ask"`. |
| `task.profile` | none | either | The [profile](#profiles) a task starts with when `kebbie start` names none with `--profile`. |
| `task.sandbox` | `true` | either | Whether a task's and the reviewer's shell commands run in Claude Code's sandbox. |
| `task.permission_mode` | none | either | The [permission mode](https://code.claude.com/docs/en/permission-modes) a task's Claude starts in. |
| `[[routines]]` | none | either | Checks kebbie runs on a schedule, each as a task that reports, plans or fixes. See [`[[routines]]`](#routines). |

`task.rules_skill`: a task's Claude follows it, the reviewer checks a ready task against it, and `tidy` reshapes commits by it. With the default, kebbie also refuses a ready report while a commit lacks a subject, a blank line and a body.

`task.test_commands`: the built-in ones are `cargo test`, `bin/rails test`, `pytest`, `npm test`, `go test` and others; `kebbie config` lists them. A command counts when it starts with one of these, whole words, after any `cd … &&` and `VAR=value`. See [`kebbie evidence`](commands.md#kebbie-evidence-name---json).

`task.also_noticed` covers things such as a bug nearby, a stale doc or a refactor. See [Decide what a task does with what it notices](../how-to/decide-what-a-task-does-with-what-it-notices.md).

- `"backlog"` files it: in the vault's backlog with the vault plugin, else with `kebbie backlog add`.
- `"fix"` fixes a small one beside the code it is changing, in a commit of its own, and files the rest.
- `"ask"` puts each to you with `kebbie ask` first.
- Each is listed under Also noticed on the sign-off card. Under `"backlog"` and `"ask"`, the reviewer counts a change outside the task as a finding.

`task.profile`: unset, tasks start with the built-in `task`. A restarted task keeps the profile it started with. See [Give tasks a profile](../how-to/give-tasks-a-profile.md).

`task.sandbox` is strict: no retry outside it, and no start without it. See [The trust boundary](../explanation/trust-boundary.md#the-sandbox).

- A task's shell writes only in its worktree, its temp folder, the package caches (`~/.cargo/registry`, `~/.cargo/git`, `~/.npm`, `~/.cache`) and the repo's shared `.git` folder, except its hooks, config and the other pooled worktrees' git dirs.
- It reaches only crates.io, static.crates.io, index.crates.io, registry.npmjs.org, github.com, objects.githubusercontent.com, pypi.org and files.pythonhosted.org, and no Unix socket.
- On macOS it may listen on local ports for a dev server.
- kebbie's own task commands (`peers`, `tell`, `ask`, `report`, `routine ok`, `link`, `backlog add`, `config`) run outside it.
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
| `review.model` | none | either | The model the reviewer runs on. |

`docs.required`: a task's Claude updates the README and `docs/` when its change alters what a user sees or does, by the [`kebbie:keep-docs`](skills.md) skill. The reviewer, in the rules check on a ready report and in `kebbie review`, fails a branch that changes such behaviour and leaves it undescribed in the README or `docs/`. `kebbie doctor` suggests it for a repo that is public on GitHub. See [Keep docs in step with changes](../how-to/keep-docs-in-step.md).

`review.model` is the `reviewer` profile's `model` by another name, and ahead of it when both are set. Unset leaves it to that profile, and then to Claude's default.

### Machine

| Key | Default | Where | What it does |
| --- | --- | --- | --- |
| `machine.name` | the host's short name | config.toml only | The machine's one name, on your phone through the hub and on the vault notes it claims. |
| `notify.command` | none | config.toml only | The shell command kebbie runs when something needs you. Unset sends nothing. See [Notify command](#notify-command). |
| `vault.command` | none | config.toml only | The notes-vault-mcp command kebbie writes task notes and log lines with, before its own arguments. |
| `hub.url` | the kebbie hub, `https://hub.kebbie.dev` | config.toml only | The [kebbie hub](../how-to/see-your-board-on-your-phone.md) `kebbie hub connect` and the board's `H` window connect this machine to. |

`machine.name`: your phone shows it through the [hub](../how-to/see-your-board-on-your-phone.md#connect-the-machine), and kebbie claims vault notes under it. See [Two machines, one vault](../explanation/vault.md#two-machines-one-vault).

- `kebbie hub connect`, the board's `H` window and `kebbie hub join` use it, set it from `--name`, or ask for it when it is unset. [`kebbie:configure`](skills.md) asks for it first when it sets kebbie up.
- A change renames the machine on the hub with the keeper's next post, or at once with `kebbie hub status`. While that fails, `kebbie hub status` and `kebbie doctor` say why.
- At most 64 characters, without control characters. `"localhost"` is refused, and `kebbie doctor` warns while the key is unset.
- A task started from a vault note, or one kebbie opens a note for, writes this name and the task's into the note's `kebbie` frontmatter key. kebbie on another machine sharing the vault shows the note as `working on <name> · <task>` and asks before starting it.
- Give each machine its own stable name, such as `"mac"` and `"devbox"`.

`vault.command`: unset runs the vault plugin's pinned command from its hooks. It is used only while the vault plugin is installed and enabled.

`hub.url` is `https`, or `http` on this machine.

- Unset, `connect` and `H` ask whether to use the kebbie hub or your own. Choosing your own tests the address and sets this key, never through a config.toml that is a link.
- The hub a machine is connected to stays with its keys in [`hub/`](files.md#the-state-directory), so a change here moves nothing. `kebbie hub move` moves it, and so does a permanent redirect (308) to another port or to `https` on the same host.
- `kebbie config` shows it, and `hub.connected` with the hub this machine posts to.

### CI, updates and the keeper

| Key | Default | Where | What it does |
| --- | --- | --- | --- |
| `ci.watch` | `true` | either | Whether kebbie watches CI after a push and closes the task only once every workflow run for the pushed commit is green. |
| `update.channel` | `"release"` | config.toml only | What `kebbie update` installs: `"release"`, the highest `vX.Y.Z` tag on origin, or `"main"`, origin's default branch. |
| `restart.auto` | `true` | config.toml only | Whether kebbie restarts the Claude sessions that run older code than is installed, after an update or a new Claude Code; no other session restarts. |
| `restart.idle_minutes` | `10` | config.toml only | How long, an integer from 1 to 1440 minutes, a session on older code waits to be idle and out of focus before that restart. |
| `keeper.enabled` | `true` | config.toml only | Whether the keeper, one background kebbie per machine, runs the routines. |
| `recover.enabled` | `true` | config.toml only | Whether kebbie resumes a task whose Claude an API error stopped. |

`ci.watch` needs origin on GitHub and `gh` logged in. `false` closes the task right after the push, as for a repo without useful CI. See [Watch CI after a push](../explanation/trunk-based.md#ci-after-the-push).

`restart.auto`: each restarted session resumes its conversation, and `kebbie update` restarts the idle ones at once. A session mid-turn, the pane you are in and one with background work running are never restarted on their own. `false` restarts nothing on its own; the board still marks the sessions on older code.

`keeper.enabled`: the keeper's routines then go on with the board closed. `false` runs them in an open board instead, and the board then posts the hub's board once a minute. See [The keeper](../explanation/updates-and-restarts.md#the-keeper). The routines are:

- held messages and delivery checks of typed prompts, stale-session restarts and the update check;
- the push lines and local-main staging, CI watches, closing landed tasks and rebasing the tasks that follow main;
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
| `report.signals` | `true` | config.toml only | Whether kebbie sends its maintainers error signals when kebbie itself fails. |
| `report.text` | `"ask"` | config.toml only | What happens to a bug in kebbie you ran into, once you or the lead saved it: `"ask"`, `"always"` or `"never"`. |
| `report.enabled` | none | config.toml only | The older single switch, still read: `false` means `signals = false` and `text = "never"`, unless those keys are set. |
| `report.inbox_token` | none | config.toml only | For kebbie's maintainers: the `kha_` token the issues endpoint issued. |
| `issues.propose` | `true` | config.toml only | Whether the lead proposes saving a [kebbie issue](../how-to/report-a-problem-with-kebbie.md) when kebbie itself cost something. |

`report.signals`: see [Error signals](../how-to/report-a-problem-with-kebbie.md#error-signals).

- A panic and some internal errors are counted.
- A signal holds only kebbie's version and commit, the OS and CPU architecture, the component (a fixed name from kebbie's code, such as `keeper.push`), the error kind (`panic`, `internal-error` or `corrupt-state`) and a count. Never an error message, a path, a repo, task or host name.
- The keeper sends at most one batch a day, and each send is a line under messages on the board.
- `false` sends none and drops what was counted.

`report.text`: see [Send it to kebbie's maintainers](../how-to/report-a-problem-with-kebbie.md#send-it-to-kebbies-maintainers).

- `"ask"`: the board shows the redacted text as it would go to kebbie's maintainers, and only your `y` sends it.
- `"always"`: once you arm it with `kebbie issue report --always` in your own terminal, kebbie sends it, redacted the same way, without asking, and says so under messages. Until then, or after `kebbie issue report --ask`, it asks as with `"ask"`.
- `"never"`: it stays on this machine. `kebbie issue report <id>` still sends one when you ask.

`report.inbox_token`: the keeper then files each report the endpoint lets through as a kebbie issue every 10 minutes, and `kebbie doctor` counts those in quarantine. `kebbie config` shows only whether it is set.

`issues.propose`: the lead proposes one at a calm moment between rounds, and saves it on your yes. `false` keeps the lead from holding, proposing or filing them; it saves one only when you ask it to. It takes effect when the lead next restarts (`L` on the board).

## Backlog sources

| Source | Items |
| --- | --- |
| `"local"` | The local backlog file, `backlog.md` next to the machine config. See [Files](files.md#the-local-backlog-file). |
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

kebbie runs `notify.command` through `sh -c`, detached, and stops it after 10 seconds. Failures go to `notify.log` in the state directory and never fail the step that sent them. A task sends at most one notification per event a minute.

The command gets:

| Variable | Value |
| --- | --- |
| `KEBBIE_EVENT` | `needs-you`, `ready`, `question`, `summary`, `ci-failed`, `ci-green`, `api-error`, `routine-findings` or `routine-failed` |
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
| `bump` | list | Files whose version lines change: a path, or `{ file = "<path>", after = "<marker>" }` to change only the first version line after the marker. |
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
| `ops` | kebbie's task whose `ssh`, `scp` and `rsync` run outside the sandbox, for work on other machines. An ssh session reaches this machine as you too, so kebbie says so when the task starts. |
| `reviewer` | The read-only reviewer `kebbie review` runs. It is never a task's profile. |

A file named `task.toml`, `ops.toml` or `reviewer.toml` adds its keys to that built-in profile.

| Key | Default | What it does |
| --- | --- | --- |
| `about` | none | One line on what the profile does. `kebbie profiles` shows it, and the task's role opens its profile section with it. |
| `for` | none | The work it fits, in your words. The lead chooses a task's profile by it. |
| `agent` | `"claude"` | The agent CLI the task runs: `"claude"` or `"codex"`. The built-in profiles run Claude. |
| `model` | none | The model, as `claude --model` or `codex --model` takes it. Unset leaves it to the agent's default. |
| `effort` | none | `"low"`, `"medium"`, `"high"`, `"xhigh"` or `"max"`, as `claude --effort` takes it; a Codex task gets it as `model_reasoning_effort`. |
| `role` | none | A markdown file in the profiles folder, carried in the task's role after kebbie's own, under `## Your profile: <name>`. |
| `skills` | `[]` | Skills the task's Claude runs once its worktree is set up, named as `task.rules_skill` is. |
| `permission_mode` | none | The permission mode, one of `task.permission_mode`'s values, ahead of it and behind `kebbie start --mode`. |
| `deny` | `[]` | Claude Code permission rules added after kebbie's own deny rules. |
| `allow` | `[]` | Claude Code permission rules added to the task's allow list. |
| `claude.plugins` | `[]` | Claude Code plugins to turn on for the task, as `<plugin>@<marketplace>`. |
| `sandbox.enabled` | `task.sandbox` | Whether this profile's tasks run sandboxed, ahead of `task.sandbox`. |
| `sandbox.allowed_domains` | kebbie's registries | The hosts a task's shell may reach, in place of kebbie's list. |
| `sandbox.allow_write` | kebbie's caches | The folders a task's shell may write besides its worktree and temp folder, in place of kebbie's list. |
| `sandbox.excluded_commands` | `[]` | Commands that run outside the sandbox, added to kebbie's own and the built-in profile's. |

- `agent = "codex"` runs OpenAI's Codex CLI, which needs kebbie's plugin installed with [`kebbie setup codex`](commands.md#setup-and-maintenance); see [Run a task with Codex](../how-to/run-a-task-with-codex.md).
- `role` is a path relative to the profiles folder: no absolute path, no `..`, and not a symlink.
- `skills`: the role names them; this is an instruction, not a guarantee.
- `permission_mode`: a Codex task gets it as Codex's sandbox and approval flags, where only `"auto"` lets Codex's own reviewer approve a command outside the sandbox; see [How the sandbox and the permission mode map](../how-to/run-a-task-with-codex.md#how-the-sandbox-and-the-permission-mode-map).
- `allow`: a rule that would let the task run `kebbie push`, `approve`, `choose`, `away`, `decide` or `after`, and a rule that allows every shell command, are refused when the profile loads. This check reads the rule's text; what keeps a task from pushing, approving or deciding is kebbie's deny rules, the push block and kebbie's own check of who calls it.
- `claude.plugins` go into the task's settings as `enabledPlugins`. The plugin must be installed.
- `sandbox.allow_write`: each folder from `/` or `~/`. A folder that holds or lies inside kebbie's state, config or install, herdr's config, `~/.claude` or a shell startup file is refused when the profile loads.
- `sandbox.excluded_commands` are in Claude Code's syntax (`docker compose *`). One that names kebbie, herdr, tmux, screen or zellij, or starts with a wildcard, is refused when the profile loads. Such a command reaches everything you can.
- `deny`, `allow`, `claude.plugins` and every `sandbox` key but `enabled` are Claude Code's: a profile with `agent = "codex"` that sets one does not load, and the error names the key.

The `reviewer` profile takes `about`, `for`, `agent`, `model`, `effort` and `role`, its `agent` only `"claude"`. The reviewer only reads, so its tools, permission mode and sandbox are kebbie's. Its sandbox follows `task.sandbox`, with no host, folder or command outside it. Its `role` goes after the review skill.

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
