# Give tasks a profile

A profile starts a task with its own model, effort, extra role, skills, permission mode and permission rules, on top of what every task gets from kelpie. Use one when a kind of work needs more than kelpie's task role: UI work that starts from a mockup, or risky changes that start in plan mode on the strongest model.

## Write a profile

1. Make the folder:

   ```bash
   mkdir -p ~/.config/kelpie/profiles
   ```

2. Write the profile, `~/.config/kelpie/profiles/designer.toml`:

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

   `for` is what the lead reads when it chooses a profile, so write it as the kinds of work the profile fits.

3. Write its role beside it, `~/.config/kelpie/profiles/designer.md`, in plain markdown: how this kind of work is done. The task's role carries it after kelpie's own, under `## Your profile: designer`.

4. Check that it loads:

   ```bash
   kelpie profiles
   ```

   A profile with a mistake is listed as ignored, with its file and key.

[`examples/profiles/`](../../examples/profiles) has a `designer` and a `careful` profile, each with its role file, to copy. [Profiles](../reference/configuration.md#profiles) lists every key. A profile with `agent = "codex"` runs the task in Codex instead; see [Run a task with Codex](run-a-task-with-codex.md).

## Start a task with it

```bash
kelpie start checkout-redesign --profile designer "Redesign the checkout page"
```

The board shows the task as `checkout-redesign · designer`, and `kelpie status --json` gives `"profile": "designer"` for it. A restart, by kelpie or by you, keeps the profile. If the profile no longer loads by then, the task restarts as the built-in `task` and kelpie names the file and the error.

The lead chooses a profile by `for` when it starts a task, and says which it chose. Ask it for one by name, or for none, as you would ask for any other part of the start.

To start every task in a repo with a profile, set it in the repo config, `~/.config/kelpie/repos/app.toml`:

```toml
[task]
profile = "designer"
```

`kelpie start --profile` still wins for one task. A routine names its own with `profile = "<name>"` in its `[[routines]]` table.

## Open the sandbox for a kind of work

A task's shell commands run in Claude Code's sandbox (see [The trust boundary](../explanation/trust-boundary.md#the-sandbox)). A profile can give its tasks other hosts, other folders to write, or commands that run outside the sandbox:

```toml
# ~/.config/kelpie/profiles/android.toml
for = "the Android app"

[sandbox]
allowed_domains = ["dl.google.com", "repo.maven.apache.org", "services.gradle.org"]
allow_write = ["~/.gradle", "~/.android"]
excluded_commands = ["adb *"]
```

`allowed_domains` and `allow_write` replace kelpie's lists, so name every host and folder the work needs. `excluded_commands` adds to kelpie's own; a command outside the sandbox reaches everything you can, so name the narrowest one. One that can run another program is a full way out of the sandbox: `ssh -o ProxyCommand=…`, `git -c core.sshCommand=…`, an interpreter, `make`, `npm run` or any tool that reads a file in the worktree, which the task can write first. `enabled = false` runs the profile's tasks unsandboxed.

For work on other machines, start the task with the built-in `ops` profile: its `ssh`, `scp` and `rsync` run outside the sandbox, which they need to connect at all.

```bash
kelpie start rotate-certs --profile ops "Rotate the TLS certificates on the web hosts"
```

An ssh session can reach this machine as you as well, past the sandbox, so `kelpie start` says so for an `ops` task. Start one only for work you would run over ssh yourself.

## What a profile cannot change

A profile only adds. Whatever it says, a task's Claude still has kelpie's role, its hooks, its deny rules on `git push` and kelpie's user-only commands (`kelpie push`, `approve`, `choose`, `after` and the rest) and on kelpie's, herdr's and Claude Code's own files, and the worktree's push block. An `allow` rule that would let a task run one of kelpie's guarded commands, or every shell command, is refused when the profile loads, as are a `sandbox.excluded_commands` entry that names kelpie, herdr or a terminal multiplexer and an `allow_write` folder that holds kelpie's, herdr's or Claude Code's own files.

`skills` puts the skills in the task's role as the first thing to run; Claude follows it like the rest of its role. `claude.plugins` turns installed plugins on for the task only.

## Change the reviewer

`kelpie review`'s reviewer is the built-in `reviewer` profile. A `~/.config/kelpie/profiles/reviewer.toml` sets its `model`, its `effort`, and a `role` that goes after the review skill:

```toml
model = "opus"
effort = "max"
role = "reviewer.md"
```

The reviewer only reads, so its file takes no permission mode, rules, skills or plugins. `review.model` in a config still sets its model, and wins over the file when both do.
