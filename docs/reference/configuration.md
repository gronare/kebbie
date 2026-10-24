# Configuration

Every key has a default, so kelpie runs without a config file. `kelpie config` prints the keys with the value in effect, the layer it comes from and what it does.

## Files and layers

kelpie reads each key from the first layer that sets it:

| Layer | File | Holds |
| --- | --- | --- |
| Repo config | `~/.config/kelpie/repos/<repo>.toml` | `[worktree]`, `[task]`, `[review]`, `[docs]`, `[[actions]]` and `[[routines]]` for one repo. `<repo>` is the main checkout's folder name, so every worktree of `~/code/app` reads `repos/app.toml`. |
| Machine config | `~/.config/kelpie/config.toml` | Every key. |
| Defaults | built in | Every key. |

[Profiles](#profiles) are files of their own, one per profile in `~/.config/kelpie/profiles/`.

`$XDG_CONFIG_HOME/kelpie` replaces `~/.config/kelpie` when it is set. `KELPIE_CONFIG`, `KELPIE_REPOS_DIR` and `KELPIE_PROFILES_DIR` override each path; see [Environment](environment.md).

kelpie never reads settings from a file inside a repo.

Rules:

- A key marked **config.toml only** in a repo config is an error that says so.
- An unknown key or table in either file is ignored, with one line on stderr that names the file and the key, so a key from a newer kelpie never stops this one. `kelpie doctor` lists them as warnings. A known key with a value of the wrong type is an error that names the file and the key. Unknown keys inside an `[[actions]]` or `[[routines]]` entry are errors, since that entry says what kelpie runs.
- `kelpie update` reads only `update.channel` from config.toml, so nothing else in the file stops an update.
- Either file may be a symlink. `kelpie config` and `kelpie doctor` show each link with its target, and `kelpie doctor` reports a broken one.

## Keys

| Key | Type | Default | Where | Meaning |
| --- | --- | --- | --- | --- |
| `board.backlog.sources` | list of `"local"`, `"vault"`, `"linear"` | `["local"]` | config.toml only | Where the board's Backlog column finds work. The first source is where the lead files new ideas. `[]` turns the backlog off. See [Backlog sources](#backlog-sources). |
| `board.backlog.ongoing` | `"top"`, `"bottom"`, `"hidden"` | `"top"` | config.toml only | Where the Backlog column shows ONGOING, the vault notes in progress: above the backlog items, below them, or not at all. Hidden, a note another machine claims still asks before it starts here. |
| `board.groups` | list of `"needs-you"`, `"working"`, `"to-review"`, `"ready-to-push"`, `"ci-running"`, `"waiting-for-work"`, `"queued"` | that order | config.toml only | The order of the Work column's groups. Groups the list leaves out follow in the default order, so `["ready-to-push"]` moves only Ready to push to the top. A name not in the list is an error that names the valid ones. |
| `worktree.setup_skill` | string | `"kelpie:set-up-worktree"` | either | The skill a task's Claude runs first to make its worktree runnable. `plugin:skill` for a plugin's skill; a bare name for one in `~/.claude/skills` or the repo's `.claude/skills`. |
| `worktree.setup_command` | string | none | either | The repo's own command that prepares a worktree, run from inside it by the setup skill. A command naming a path applies only in a repo that has that file. Where it does not apply, and when unset, the skill runs `bin/setup-worktree` if the repo has one. |
| `worktree.dev_server` | string | none | either | The command the board's `w` runs in a server tab of the task's Space, with `PORT` set to the worktree's port. Unset, there is no `w`. |
| `worktree.dev_worker` | string | none | either | A second command, such as a job runner, that `b` runs in a worker tab, with the same `PORT`. Unset, there is no `b`. |
| `worktree.dev_url` | string | `"http://localhost:{port}"` | either | The running dev server's address, shown on the board as a link. See [URL placeholders](#url-placeholders). `""` turns the link off. |
| `worktree.dev_route` | boolean | `false` | repo config only | Whether [`kelpie routes`](commands.md#setup-and-maintenance) writes a Caddy site for each of this repo's worktrees, at the host `dev_url` names, proxied to the worktree's port. Opt-in per repo, so a repo that serves its worktrees itself stays out. See [Open dev servers from a remote machine](../how-to/open-dev-servers-from-a-remote-machine.md). |
| `worktree.stage_on_main` | boolean | `false` | repo config only | Whether [`kelpie stage`](commands.md#kelpie-stage-name) (`M` on the board) may put a ready task's commits on this repo's local default branch, unpushed, to try them from the main checkout before you push main. For a repo you run from its main checkout; kelpie's own repo, when you run kelpie from your clone, turns it on. See [Try a task before pushing](../how-to/try-a-task-before-pushing.md). |
| `worktree.clean_on_release` | list of strings | `["target"]` | either | Build output kelpie removes from a worktree when it goes back to the pool (`done`, `drop`, `reset`, a landed task the keeper closes), so the pool's disk use stays bounded; the result says what it freed, as `freed: removed target/ (3.7 GB)`. Folders inside the worktree, such as `"node_modules/.cache"`, `".next"` or `"dist"`; each is removed only when git ignores it and tracks nothing in it, a symlink is never followed, and `"target"` only where the worktree has a `Cargo.toml`. `[]` keeps everything. |
| `worktree.server_name` | string | `"server"` | config.toml only | The name of the tab `w` starts, and of the Space's tag in herdr's sidebar while it runs. |
| `worktree.worker_name` | string | `"worker"` | config.toml only | The same for `b`'s tab. |
| `task.rules_skill` | string | `"kelpie:work-rules"` | either | The skill with the rules for how work is planned, built and committed. A task's Claude follows it, the reviewer checks a ready task against it, and `tidy` reshapes commits by it. With the default, kelpie also refuses a ready report while a commit lacks a subject, a blank line and a body. |
| `task.test_commands` | list of strings | `[]` | either | Test commands that kelpie records when a task's Claude runs them, on top of the built-in ones (`cargo test`, `bin/rails test`, `pytest`, `npm test`, `go test` and others; `kelpie config` lists them). A command counts when it starts with one of these, whole words, after any `cd … &&` and `VAR=value`. See [`kelpie evidence`](commands.md#kelpie-evidence-name---json). |
| `task.also_noticed` | `"backlog"`, `"fix"` or `"ask"` | `"backlog"` | either | What a task's Claude does with what it notices outside its task, such as a bug nearby, a stale doc or a refactor. `"backlog"` files it: in the vault's backlog with the vault plugin, else with `kelpie backlog add`. `"fix"` fixes a small one beside the code it is changing, in a commit of its own, and files the rest. `"ask"` puts each to you with `kelpie ask` first. Each is listed under Also noticed on the sign-off card, and the reviewer counts a change outside the task as a finding under `"backlog"` and `"ask"`. See [Decide what a task does with what it notices](../how-to/decide-what-a-task-does-with-what-it-notices.md). |
| `task.profile` | string | none | either | The [profile](#profiles) a task starts with when `kelpie start` names none with `--profile`. Unset, tasks start with the built-in `task`. A restarted task keeps the profile it started with. See [Give tasks a profile](../how-to/give-tasks-a-profile.md). |
| `task.sandbox` | boolean | `true` | either | Whether a task's and the reviewer's shell commands run in Claude Code's sandbox, strict: no retry outside it, and no start without it. A task's shell then writes only in its worktree, its temp folder, the package caches (`~/.cargo/registry`, `~/.cargo/git`, `~/.npm`, `~/.cache`) and the repo's shared `.git` folder except its hooks, config and the other pooled worktrees' git dirs, reaches only crates.io, static.crates.io, index.crates.io, registry.npmjs.org, github.com, objects.githubusercontent.com, pypi.org and files.pythonhosted.org, and no Unix socket; on macOS it may listen on local ports for a dev server. kelpie's own task commands (`peers`, `tell`, `ask`, `report`, `routine ok`, `link`, `backlog add`, `config`) run outside it. On Linux it needs bubblewrap, socat and the seccomp filter (`npm install -g @anthropic-ai/sandbox-runtime`); without them kelpie refuses to start a task or a review and names what to install. `false` runs them unsandboxed, where a task's shell can pass as you to kelpie's user-only commands; `kelpie doctor`, a task's start and the board's Areas page say so. A profile's `[sandbox] enabled` wins over it. See [The trust boundary](../explanation/trust-boundary.md#the-sandbox). |
| `task.permission_mode` | `"default"`, `"acceptEdits"`, `"plan"`, `"auto"`, `"dontAsk"` or `"bypassPermissions"` | none | either | The [permission mode](https://code.claude.com/docs/en/permission-modes) a task's Claude starts in. Unset, Claude Code's own default applies. `kelpie start --mode` and then the task's profile's `permission_mode` override it, and a restarted task keeps the mode it started in. Any other value is refused when the config loads. `"bypassPermissions"` runs every tool without asking; Claude Code recommends it only for an isolated container or VM. A task still never pushes nor approves its own commits in it, since kelpie's deny rules and the worktree's push block hold in every mode. See [Let tasks run without permission prompts](../how-to/let-tasks-run-without-prompts.md). |
| `docs.required` | boolean | `false` | repo config only | Whether this repo's docs keep in step with its code. A task's Claude updates the README and `docs/` when its change alters what a user sees or does, by the [`kelpie:keep-docs`](skills.md) skill, and the reviewer, in the rules check on a ready report and in `kelpie review`, fails a branch that changes such behaviour and leaves it undescribed in the README or `docs/`. `kelpie doctor` suggests it for a repo that is public on GitHub. See [Keep docs in step with changes](../how-to/keep-docs-in-step.md). |
| `review.model` | string | none | either | The model the reviewer runs on: the `reviewer` profile's `model` by another name, and ahead of it when both are set. Unset leaves it to that profile, and then to Claude's default. |
| `notify.command` | string | none | config.toml only | The shell command kelpie runs when something needs you. See [Notify command](#notify-command). Unset sends nothing. |
| `vault.command` | string | none | config.toml only | The notes-vault-mcp command kelpie writes task notes and log lines with, before its own arguments. Unset runs the vault plugin's pinned command from its hooks. Used only while the vault plugin is installed and enabled. |
| `machine.name` | string | the host's short name | config.toml only | The machine's one name: your phone shows it through the [hub](../how-to/see-your-board-on-your-phone.md#connect-the-machine), and kelpie claims vault notes under it. `kelpie hub connect`, the board's `H` window and `kelpie hub join` use it, set it from `--name`, or ask for it when it is unset, and [`kelpie:configure`](skills.md) asks for it first when it sets kelpie up. A change renames the machine on the hub with the keeper's next post, or at once with `kelpie hub status`; while that fails, `kelpie hub status` and `kelpie doctor` say why. At most 64 characters, without control characters. A task started from a vault note, or one kelpie opens a note for, writes this name and the task's into the note's `kelpie` frontmatter key, and kelpie on another machine sharing the vault shows the note as `working on <name> · <task>` and asks before starting it. Give each machine its own stable name, such as `"mac"` and `"devbox"`; `"localhost"` is refused, and `kelpie doctor` warns while the key is unset. See [Two machines, one vault](../explanation/vault.md#two-machines-one-vault). |
| `ci.watch` | boolean | `true` | either | Whether kelpie watches CI after a push and closes the task only once every workflow run for the pushed commit is green. Needs origin on GitHub and `gh` logged in. `false` closes the task right after the push, as for a repo without useful CI. See [Watch CI after a push](../explanation/trunk-based.md#ci-after-the-push). |
| `update.channel` | `"release"` or `"main"` | `"release"` | config.toml only | What `kelpie update` installs: the highest `vX.Y.Z` tag on origin, or origin's default branch. |
| `restart.auto` | boolean | `true` | config.toml only | Whether kelpie restarts stale idle Claude sessions on its own and `kelpie update` restarts idle ones at once. `false` restarts nothing on its own; the board still marks stale sessions. |
| `restart.idle_minutes` | integer, 1 to 1440 | `10` | config.toml only | How long a stale session must be idle and out of focus before it is restarted. |
| `keeper.enabled` | boolean | `true` | config.toml only | Whether the keeper, one background kelpie per machine, runs the routines: held messages and delivery checks of typed prompts, stale-session restarts, the update check, the push lines and local-main staging, CI watches, closing landed tasks, rebasing the tasks that follow main, relaying a task's Claude's own prompts to the lead, resuming tasks an API error stopped, the rules check on a ready report, rechecking kelpie issues after an update, the scheduled `[[routines]]`, starting the queued tasks whose wait is over, posting the board to a [kelpie hub](../how-to/see-your-board-on-your-phone.md), sending error signals and bug reports to kelpie's maintainers, and with `report.inbox_token` fetching the reports inbox. They then go on with the board closed. `false` runs them in an open board instead; the board then posts the hub's board once a minute. See [The keeper](../explanation/updates-and-restarts.md#the-keeper). |
| `hub.url` | string | the kelpie hub, `https://kelpie.gronare.com` | config.toml only | The [kelpie hub](../how-to/see-your-board-on-your-phone.md) `kelpie hub connect` and the board's `H` window connect this machine to, `https` (or `http` on this machine). Unset, they ask whether to use the kelpie hub or your own; choosing your own tests the address and sets this key, never through a config.toml that is a link. The hub a machine is connected to stays with its keys in [`hub/`](files.md#the-state-directory), so a change here moves nothing: `kelpie hub move` moves it, and so does a permanent redirect (308) to another port or to `https` on the same host. `kelpie config` shows it, and `hub.connected` with the hub this machine posts to. |
| `report.signals` | boolean | `true` | config.toml only | Whether kelpie sends its maintainers error signals when kelpie itself fails. A panic and some internal errors are counted. A signal holds only kelpie's version and commit, the OS and CPU architecture, the component (a fixed name from kelpie's code, such as `keeper.push`), the error kind (`panic`, `internal-error` or `corrupt-state`) and a count; never an error message, a path, a repo, task or host name. The keeper sends at most one batch a day, and each send is a line under messages on the board. `false` sends none and drops what was counted. See [Error signals](../how-to/report-a-problem-with-kelpie.md#error-signals). |
| `report.text` | `"ask"`, `"always"`, `"never"` | `"ask"` | config.toml only | What happens to a bug in kelpie you ran into, once you or the lead saved it. `"ask"`: the board shows the redacted text as it would go to kelpie's maintainers and only your `y` sends it. `"always"`: once you arm it with `kelpie issue report --always` in your own terminal, kelpie sends it, redacted the same way, without asking, and says so under messages; until then, or after `kelpie issue report --ask`, it asks as with `"ask"`. `"never"`: it stays on this machine; `kelpie issue report <id>` still sends one when you ask. See [Send it to kelpie's maintainers](../how-to/report-a-problem-with-kelpie.md#send-it-to-kelpies-maintainers). |
| `report.enabled` | boolean | none | config.toml only | The older single switch, still read: `false` means `signals = false` and `text = "never"`, unless those keys are set. |
| `report.url` | string | the project's reports endpoint | config.toml only | Where reports go, `https` (or `http` on this machine). |
| `report.inbox_token` | string | none | config.toml only | For whoever runs the reports endpoint: the `kha_` token `kelpie-hub admin token --name <name>` printed. The keeper then files each report the hub lets through as a kelpie issue every 10 minutes, and `kelpie doctor` counts those in quarantine. `kelpie config` shows only whether it is set. |
| `issues.propose` | boolean | `true` | config.toml only | Whether the lead proposes saving a [kelpie issue](../how-to/report-a-problem-with-kelpie.md) when kelpie itself cost something, one at a calm moment between rounds, and saves it on your yes. `false` keeps the lead from holding, proposing or filing them; it saves one only when you ask it to. Takes effect when the lead next restarts (`L` on the board). |
| `recover.enabled` | boolean | `true` | config.toml only | Whether kelpie resumes a task whose Claude an API error stopped. After overload, a server error, a rate limit, a timeout or a dropped connection it tells the task to carry on after 1, 2, 4 and 8 minutes; after a usage limit, once the allowance resets. Login, billing, invalid request and refusal errors go to you under Needs you at once. The schedule is fixed. See [When the API stops a task](../explanation/lead-and-away-mode.md#when-the-api-stops-a-task). |
| `[[routines]]` | tables | none | either | Checks kelpie runs on a schedule, each as a task that reports, plans or fixes. See [`[[routines]]`](#routines). |
| `[keys]` | table of strings | the board's own | config.toml only | The board's keys by action, as `inbox = "I"`. See [Board](board.md#keys-in-configtoml) for the actions and the rules; `kelpie config` shows each key in effect and why an entry was ignored. |

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

kelpie runs `notify.command` through `sh -c`, detached, and stops it after 10 seconds. Failures go to `notify.log` in the state directory and never fail the step that sent them. A task sends at most one notification per event a minute.

The command gets:

| Variable | Value |
| --- | --- |
| `KELPIE_EVENT` | `needs-you`, `ready`, `question`, `summary`, `ci-failed`, `ci-green`, `api-error`, `routine-findings` or `routine-failed` |
| `KELPIE_TASK` | The task's name |
| `KELPIE_REPO` | The task's repo |
| `KELPIE_TEXT` | One line, such as `billing@app needs you: answer which queue` |
| `$1` | The same text |

| Event | Sent when |
| --- | --- |
| `needs-you` | A task reports that it needs you, or a routine's Claude asks whether you trust its folder. |
| `ready` | A task reports ready. |
| `question` | A task asks you a question. |
| `summary` | You or the lead run `kelpie notify "<text>"`. Tasks cannot. |
| `ci-failed` | A workflow run for a pushed commit failed. The text names the workflow and the job. |
| `ci-green` | Every workflow run for a pushed commit passed, or none ran. |
| `api-error` | An API error stopped a task and kelpie will not resume it: a login, billing, invalid request or refusal error, or one that persisted through every retry. The text gives the reason and Claude Code's words. |
| `routine-findings` | A routine's task found something: its first report or question, in place of that report's `needs-you`, `ready` or `question`. A run that ends with `kelpie routine ok` sends nothing. |
| `routine-failed` | A routine's run could not start, or its task closed before it reported. |

## `[[actions]]`

Repo config only. Each table adds an action to the repo's row on the board's Areas page.

| Key | Type | Meaning |
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
| `task` | string | Start a kelpie task with this prompt instead of running a script. |
| `then` | list of `"repo:action"` | Actions offered once this one is done. Never run on their own. |

A script action runs only in a main checkout that is on the default branch, clean and not behind origin. The window shows every step first, including the exact lines a bump changes, and nothing changes until you press `y`. A bump touches only lines that mention a version and hold the current one. Paths stay inside the repo.

Example:

```toml
# ~/.config/kelpie/repos/billing.toml
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

Machine config or repo config. Each table is a check kelpie runs on a schedule as a task. The board's [Routines page](board.md#routines-page) and `kelpie routine add` write new ones into config.toml, and edit, pause and delete them in the file they are in. See [Routines](../explanation/routines.md) and [Add a routine](../how-to/add-a-routine.md).

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `id` | string | required | The routine's name: lower-case letters, digits and single dashes, starting with a letter, at most 40. Unique across config.toml and every repo config. Its runs' tasks are named `<id>-<yyyymmdd-hhmm>`, the id cut to fit herdr's 32 characters. |
| `title` | string | required | What `kelpie routines` and the run's prompt call it. |
| `repo` | string | the repo config's repo | The repo the run's task starts in: a checkout's folder name, as `kelpie start --repo` takes it, or its path (`~/code/infra`). Required in config.toml. |
| `every` | string | | An interval: `"30m"`, `"6h"`, `"1d"`. A run is due once it has passed since the last run started. |
| `at` | string | | A local time: `"07:00"` every day, or `"mon 07:00"` once a week (`mon` to `sun`). A run is due once that time has come since the last run started. Give `every` or `at`, not both. |
| `prompt` | string | required | What to check, in your words. |
| `mode` | `"report"`, `"plan"` or `"fix"` | `"report"` | What the task does with what it finds: report it, propose a plan for your approval, or fix it in commits. |
| `push` | boolean | `false` | Fix routines only: the run's task carries the push permission the board's `a` gives, so the lead pushes its fix once its commits hold to the rules. |
| `enabled` | boolean | `true` | `false` keeps the routine off the schedule; `kelpie routine run <id>` still starts it. |
| `profile` | string | `task.profile`, else `task` | The [profile](#profiles) the run's task starts with. A routine's task runs in `auto` mode whatever the profile's `permission_mode` says. |

A routine that is not valid is an error that names its file, its index and the key, as for every other key. `kelpie config` lists the routines it found.

Example:

```toml
# ~/.config/kelpie/repos/infra.toml
[[routines]]
id = "cluster-health"
title = "Cluster health"
every = "6h"
prompt = "Check that every node is Ready, no pod is crash-looping and every volume is healthy."
```

## Profiles

A profile names the agent, model, extra role, skills, permission mode and rules a task starts with. Each is a file, `~/.config/kelpie/profiles/<name>.toml`, its name lower-case letters, digits and dashes, starting with a letter. A task gets kelpie's own role, hooks, deny rules and push block whatever its profile says; a profile only adds to them.

Three are built in:

| Profile | What it is |
| --- | --- |
| `task` | kelpie's task as it is: nothing added. Tasks start with it unless `--profile` or `task.profile` names another. |
| `ops` | kelpie's task whose `ssh`, `scp` and `rsync` run outside the sandbox, for work on other machines. An ssh session reaches this machine as you too, so kelpie says so when the task starts. |
| `reviewer` | The read-only reviewer `kelpie review` runs. It is never a task's profile. |

A file named `task.toml`, `ops.toml` or `reviewer.toml` adds its keys to that built-in profile.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `about` | string | none | One line on what the profile does. `kelpie profiles` shows it, and the task's role opens its profile section with it. |
| `for` | string | none | The work it fits, in your words. The lead chooses a task's profile by it. |
| `agent` | `"claude"` or `"codex"` | `"claude"` | The agent CLI the task runs. `"codex"` runs OpenAI's Codex CLI, which needs kelpie's plugin installed with [`kelpie setup codex`](commands.md#setup-and-maintenance); see [Run a task with Codex](../how-to/run-a-task-with-codex.md). The built-in profiles run Claude. |
| `model` | string | none | The model, as `claude --model` or `codex --model` takes it. Unset leaves it to the agent's default. |
| `effort` | `"low"`, `"medium"`, `"high"`, `"xhigh"` or `"max"` | none | The effort level, as `claude --effort` takes it; a Codex task gets it as `model_reasoning_effort`. |
| `role` | string | none | A markdown file in the profiles folder, its path relative to it: no absolute path, no `..`, and not a symlink. The task's role carries it after kelpie's own, under `## Your profile: <name>`. |
| `skills` | list of strings | `[]` | Skills the task's Claude runs once its worktree is set up, named as `task.rules_skill` is. The role names them; this is an instruction, not a guarantee. |
| `permission_mode` | the values of `task.permission_mode` | none | The permission mode, ahead of `task.permission_mode` and behind `kelpie start --mode`. A Codex task gets it as Codex's sandbox and approval flags, where only `"auto"` lets Codex's own reviewer approve a command outside the sandbox; see [How the sandbox and the permission mode map](../how-to/run-a-task-with-codex.md#how-the-sandbox-and-the-permission-mode-map). |
| `deny` | list of strings | `[]` | Claude Code permission rules added after kelpie's own deny rules. |
| `allow` | list of strings | `[]` | Claude Code permission rules added to the task's allow list. A rule that would let the task run `kelpie push`, `approve`, `choose`, `away`, `decide` or `after`, and a rule that allows every shell command, are refused when the profile loads. This check reads the rule's text; what keeps a task from pushing, approving or deciding is kelpie's deny rules, the push block and kelpie's own check of who calls it. |
| `claude.plugins` | list of strings | `[]` | Claude Code plugins to turn on for the task, as `<plugin>@<marketplace>`; they go into the task's settings as `enabledPlugins`. The plugin must be installed. |
| `sandbox.enabled` | boolean | `task.sandbox` | Whether this profile's tasks run sandboxed, ahead of `task.sandbox`. |
| `sandbox.allowed_domains` | list of strings | kelpie's registries | The hosts a task's shell may reach, in place of kelpie's list. |
| `sandbox.allow_write` | list of strings | kelpie's caches | The folders a task's shell may write besides its worktree and temp folder, in place of kelpie's list; each from `/` or `~/`. A folder that holds or lies inside kelpie's state, config or install, herdr's config, `~/.claude` or a shell startup file is refused when the profile loads. |
| `sandbox.excluded_commands` | list of strings | `[]` | Commands that run outside the sandbox, added to kelpie's own and the built-in profile's, in Claude Code's syntax (`docker compose *`). One that names kelpie, herdr, tmux, screen or zellij, or starts with a wildcard, is refused when the profile loads. Such a command reaches everything you can. |

`deny`, `allow`, `claude.plugins` and every `sandbox` key but `enabled` are Claude Code's: a profile with `agent = "codex"` that sets one does not load, and the error names the key.

The `reviewer` profile takes `about`, `for`, `agent`, `model`, `effort` and `role`, its `agent` only `"claude"`: the reviewer only reads, so its tools, permission mode and sandbox are kelpie's; its sandbox follows `task.sandbox`, with no host, folder or command outside it. Its `role` goes after the review skill.

A profile that does not load is an error that names its file and key, and a task asked to start with it does not start. A running task whose recorded profile no longer loads, when it is restarted, goes on as the built-in `task`, and kelpie says so with the file and the error: in `kelpie start`'s answer, or to the lead for a restart by `kelpie restart` or the keeper. `kelpie profiles` lists the profiles with what each is for, and names a file it ignored with the reason.

```toml
# ~/.config/kelpie/profiles/designer.toml
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
# ~/.config/kelpie/config.toml
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
command = 'curl -s -d "$KELPIE_TEXT" https://ntfy.sh/my-kelpie-topic'

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
# ~/.config/kelpie/repos/app.toml
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

| Option | Default | Meaning |
| --- | --- | --- |
| `auto_open` | `false` | Open the review split next to a task's Claude on its first file edit in a session, without taking focus. Only inside herdr and in a git worktree. Closing the pane keeps it closed for that session. |

Set it under `/plugin` → Installed → kelpie → Configure, or at install with `claude plugin install kelpie@kelpie --config auto_open=true`.
