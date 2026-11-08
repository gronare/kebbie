# Give tasks a profile

Start a kind of task with its own model, effort, extra role, skills, permission mode and permission rules, on top of what every task gets from kebbie.

Use a profile when a kind of work needs more than kebbie's task role: UI work that starts from a mockup, or risky changes that start in plan mode on the strongest model.

## Add a profile on the board

1. Open the board's Settings page with `,` and go to Profiles.
2. Select the profile to start from and press `n`.
3. Answer the seven steps: a name, what it is for (Tasks), the profile to start from, the agent (Claude Code, or Codex once you have set it up), its model, its effort and its role. `←` goes back a step, `esc` cancels and writes nothing.
4. Read the check, the file and the role file kebbie will write, and press `y` to add it, or `g` to add it and open its role in your editor.

The profile is `~/.config/kebbie/profiles/<name>.toml`, yours: an update never writes it. It takes the start's `about`, `for`, skills, permissions and sandbox, and a role of its own, a new empty file, a copy, or a role file other profiles already read. Edit `for` afterwards with `⏎`, as the lead chooses by it. `x` on the row deletes it again.

## Write a profile

1. Make the folder:

   ```bash
   mkdir -p ~/.config/kebbie/profiles
   ```

2. Write the profile, `~/.config/kebbie/profiles/designer.toml`. The lead reads `for` when it chooses a profile, so write it as the kinds of work the profile fits.

   ```toml
   about = "UI work: a mockup first, then the code, measured in the browser"
   for = "UI, CSS, new screens and components, mockups"
   model = "opus"
   effort = "high"
   role = "designer.md"
   skills = ["frontend-design:frontend-design"]
   permission_mode = "acceptEdits"
   allow = ["Bash(npx playwright:*)"]
   ```

3. Write its role beside it, `~/.config/kebbie/profiles/designer.md`, in plain markdown: how this kind of work is done. The task's role carries it after kebbie's own, under `## Your profile: designer`.

4. Check that it loads. A profile with a mistake is listed as ignored, with its file and key.

   ```bash
   kebbie profiles
   ```

[`examples/profiles/`](https://github.com/gronare/kebbie/tree/main/examples/profiles) has a `designer` and a `careful` profile, each with its role file, to copy. [Profiles](../reference/configuration.md#profiles) lists every key.

To change only a profile's model, effort or role, open the board's Settings page with `,`, go to Profiles and press `m`, `e` or `r`. See [Settings page](../reference/board.md#profiles).

## Add your lines to kebbie's task role

kebbie's own `task`, `reviewer` and `ops` profiles take a role file of yours too, read after kebbie's role, which comes with kebbie and changes with each update.

1. Open the board's Settings page with `,` and go to Profiles.
2. Select `task` under FROM KEBBIE and press `r`, then `a`.
3. Write your lines in the file that opens, `~/.config/kebbie/profiles/task.md`.

kebbie writes `role = "task.md"` in `~/.config/kebbie/profiles/task.toml`. Both files are yours, and an update never writes them. `s` in the same menu shows kebbie's own role, read-only.

If you want the task to run in Codex, give the profile `agent = "codex"`. See [Run a task with Codex](run-a-task-with-codex.md). Codex is offered only once you have set it up with `kebbie setup codex`; until then a Codex profile shows `Codex is not set up: kebbie setup codex`, and no task starts with it.

## Start a task with it

1. Start the task with `--profile`:

   ```bash
   kebbie start checkout-redesign --profile designer "Redesign the checkout page"
   ```

2. The board shows the task as `checkout-redesign · designer`, and `kebbie status --json` gives `"profile": "designer"` for it.

A restart, by kebbie or by you, keeps the profile. If the profile no longer loads by then, the task restarts as the built-in `task`, and kebbie names the file and the error.

If the lead starts the task, it chooses a profile by `for` and says which it chose. Ask it for one by name, or for none, as you would ask for any other part of the start.

If you want every task in a repo to start with a profile, set it in the repo config, `~/.config/kebbie/repos/app.toml`. `kebbie start --profile` still wins for one task.

```toml
[task]
profile = "designer"
```

If a routine needs one, name it with `profile = "<name>"` in its `[[routines]]` table.

## Open the sandbox for a kind of work

A task's shell commands run in Claude Code's sandbox (see [The trust boundary](../explanation/trust-boundary.md#the-sandbox)). A profile's `[sandbox]` table can give its tasks other hosts, other folders to write, or commands that run outside the sandbox:

```toml
# ~/.config/kebbie/profiles/android.toml
for = "the Android app"

[sandbox]
allowed_domains = ["dl.google.com", "repo.maven.apache.org", "services.gradle.org"]
allow_write = ["~/.gradle", "~/.android"]
excluded_commands = ["adb *"]
```

| Key | What it does |
| --- | --- |
| `allowed_domains` | Replaces kebbie's list of hosts, so name every host the work needs. |
| `allow_write` | Replaces kebbie's list of folders to write, so name every folder the work needs. |
| `excluded_commands` | Adds to kebbie's own commands that run outside the sandbox. |
| `enabled = false` | Runs the profile's tasks unsandboxed. |

> **Warning:** A command outside the sandbox reaches everything you can, so name the narrowest one. One that can run another program is a full way out of the sandbox: `ssh -o ProxyCommand=…`, `git -c core.sshCommand=…`, an interpreter, `make`, `npm run`, or any tool that reads a file in the worktree, which the task can write first.

## Work on other machines with ops

Start work on other machines with the built-in `ops` profile. Its `ssh`, `scp` and `rsync` run outside the sandbox, which they need to connect at all.

```bash
kebbie start rotate-certs --profile ops "Rotate the TLS certificates on the web hosts"
```

An ssh session can reach this machine as you as well, past the sandbox, so `kebbie start` says so for an `ops` task. Start one only for work you would run over ssh yourself.

## Manage a cluster

A profile reaches a Kubernetes cluster only when it names a kubeconfig file. Its tasks' `kubectl` and `helm` then run outside the sandbox with that file alone:

```toml
# ~/.config/kebbie/profiles/cluster-read.toml
for = "reading the cluster's state"

[sandbox]
kubeconfig = "~/.kube/kebbie-cluster-read.yaml"
```

What the task may do in the cluster is what that login's RBAC allows. [Manage a cluster from a scheduled task](manage-a-cluster-from-a-scheduled-task.md) sets up a login per level and the routines that use them.

## What a profile cannot change

A profile only adds. Whatever it says, a task's Claude still has:

- kebbie's role and its hooks.
- Its deny rules on `git push`, on kebbie's user-only commands (`kebbie push`, `approve`, `choose`, `after` and the rest), and on kebbie's, herdr's and Claude Code's own files.
- The worktree's push block.

When the profile loads, kebbie refuses:

- An `allow` rule that would let a task run one of kebbie's guarded commands, or every shell command.
- A `sandbox.excluded_commands` entry that names kebbie, herdr or a terminal multiplexer, or `kubectl` or `helm`, which take `sandbox.kubeconfig` instead.
- An `allow_write` folder that holds kebbie's, herdr's or Claude Code's own files.

`skills` puts the skills in the task's role as the first thing to run, and Claude follows it like the rest of its role. `claude.plugins` turns installed plugins on for the task only.

## Change the reviewer

The rules check that starts when a task reports ready runs as the built-in `reviewer` profile, and so does the full review `kebbie review` asks for unless [`review.profile`](../reference/configuration.md#review-and-docs) names another. A `~/.config/kebbie/profiles/reviewer.toml` sets its `model`, its `effort`, and a `role` that goes after the review skill:

```toml
model = "opus"
effort = "max"
role = "reviewer.md"
```

The reviewer only reads, so its file takes no permission mode, rules, skills or plugins. `review.model` in a config still sets the model of the reviewer `review.profile` names, and wins over its file when both do. The built-in `reviewer` stays on Claude.

## Add a review profile

A review profile is a reviewer of your own: another model, another effort, a role of its own, or Codex. It reviews when [`review.profile`](../reference/configuration.md#review-and-docs) names it, or gives a second opinion when `review.second` does.

1. On the Settings page, go to Profiles and press `n`.
2. Type its name, then answer Review to "What is it for?":

   ```
   ╭ new profile ───────────────────────────────────────────────────────╮
   │ step 2 of 7                                                        │
   │ What is it for?                                                    │
   │ A review profile only reads: it gets no permissions, skills or     │
   │ sandbox keys, and kebbie review can run it.                        │
   │                                                                    │
   │  Tasks               does the work in a worktree                   │
   │ ▌Review              reads a task's commits and gives a verdict    │
   ```

3. Pick the review profile it starts from, its agent, model, effort and role, then `y`.

Or write it, `~/.config/kebbie/profiles/codex-review.toml`:

```toml
kind = "review"
about = "A second reader on GPT"
agent = "codex"
model = "gpt-6.1-sol"
effort = "high"
```

- `kind = "review"` makes it a review profile. It takes `about`, `for`, `agent`, `model`, `effort` and `role` alone; any other key is refused when it loads.
- A review profile never starts a task: `kebbie start --profile` refuses it, and `task.profile` does not offer it.
- A Codex review profile runs in Codex's read-only sandbox without your Codex config. See [The reviewer only reads](../explanation/review-gate.md#the-reviewer-only-reads).
- The Settings page offers a review profile only when setup chose its agent.

Then pick it on the Settings page for `review.profile` or `review.second`. See [Ask for a second opinion](send-a-task-to-review.md#ask-for-a-second-opinion).
