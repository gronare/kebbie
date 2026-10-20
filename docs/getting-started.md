# Getting started

kelpie has three parts: the `kelpie` command (one Rust binary), a herdr plugin that builds and runs it, and a Claude Code plugin with the skills and the review hook. This page installs all three and runs your first task.

## 1. Install the tools

kelpie runs inside herdr and needs these on your `PATH`:

| Tool | Needed for | macOS | Linux |
| --- | --- | --- | --- |
| `git` | everything | `xcode-select --install` | `sudo apt install git` |
| `herdr` 0.9.0+ | everything | `brew install herdr` | `curl -fsSL https://herdr.dev/install.sh \| sh` |
| `claude` (Claude Code) | everything | `curl -fsSL https://claude.ai/install.sh \| bash` | the same |
| `jq` | the sidebar sync and the review hook | `brew install jq` | `sudo apt install jq` |
| `cargo` | building the plugin | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh` | the same |
| `fzf` 0.71+ | the review split's file list | `brew install fzf` | `git clone --depth 1 https://github.com/junegunn/fzf.git ~/.fzf && ~/.fzf/install` |
| `nvim` 0.10+ | the review split's diffs | `brew install neovim` | a [release](https://github.com/neovim/neovim/releases) |
| `curl` | the review list reloading on change | `brew install curl` | `sudo apt install curl` |
| `tig` | the history split | `brew install tig` | `sudo apt install tig` |

Distro packages of `fzf` and `nvim` are often older than these versions.

## 2. Install the herdr plugin

```bash
herdr plugin install gronare/kelpie
```

herdr clones the repo and runs `cargo build --release`. Then restart herdr. On startup the plugin links `~/.local/share/kelpie/current` to wherever herdr installed it, which gives configs and hooks a path that survives updates. Keep `XDG_DATA_HOME` unset for herdr, since kelpie reads the link under `~/.local/share`.

To work on kelpie itself, link a checkout instead: `herdr plugin link ~/code/kelpie`, then `cargo build --release` in it.

## 3. Put `kelpie` on your PATH

The lead, tasks and hooks run `kelpie` by name:

```bash
mkdir -p ~/.local/bin
ln -s ~/.local/share/kelpie/current/target/release/kelpie ~/.local/bin/kelpie
```

## 4. Install the Claude Code plugin

It brings the skills and the review hook. Add it from the install root, so it is always the same code as the binary:

```bash
claude plugin marketplace add ~/.local/share/kelpie/current
claude plugin install kelpie@kelpie
```

## 5. Add the herdr keys

In herdr's `config.toml`:

```toml
[[keys.command]]
key = "prefix+space"
type = "plugin_action"
command = "kelpie.lead"
description = "kelpie: go to the lead, or back"

[[keys.command]]
key = "prefix+tab"
type = "plugin_action"
command = "kelpie.focus"
description = "kelpie: between the board and the lead's chat"

[[keys.command]]
key = "prefix+t"
type = "plugin_action"
command = "kelpie.board"
description = "kelpie board"

[[keys.command]]
key = "prefix+u"
type = "plugin_action"
command = "kelpie.update"
description = "kelpie: update and reload"
```

[`examples/herdr-config.toml`](../examples/herdr-config.toml) is a complete herdr config with these keys, the sidebar sync and a sidebar layout that shows each task's state and your next step.

To show the sidebar tags (`server`, `worker`, the task's next step, model and context), add the sync to herdr's tab bar:

```toml
[ui]
tab_bar_right = [
  { type = "command", command = "~/.local/share/kelpie/current/scripts/sync.sh", interval_seconds = 3, timeout_seconds = 3 },
]
```

## 6. Optional: plan usage on the board

The board can show how much of your Claude plan's five-hour session and weekly limit you have used. Claude Code passes those numbers only to a status line command, so make kelpie yours in `~/.claude/settings.json`:

```json
"statusLine": { "type": "command", "command": "~/.local/share/kelpie/current/target/release/kelpie statusline" }
```

It prints nothing, so Claude Code shows no extra line. The numbers come only on a Claude.ai plan, not with an API key.

## 7. Check

```bash
kelpie doctor
```

Follow its fixes until every line is `ok`. See [Troubleshooting](troubleshooting.md) for what it checks.

## Your first task

Press `prefix+space`. The first time, kelpie creates the lead Space: the board on the left and the lead's Claude on the right. `prefix+tab` moves focus between them.

Tell the lead what you want done. It writes the task prompt and starts the task. Or start one yourself from a repo's main checkout:

```bash
cd ~/code/app
kelpie start fix-login "The session drops on reload. Find out why and fix it."
```

kelpie puts the worktree `wt1.app` next to your checkout on a new branch `fix-login`, opens it as a Space and starts Claude there. Claude first makes the worktree runnable (see [Worktrees](worktrees.md)). At the end of every turn it reports where it stands, and the board shows that under the task.

When the task reports ready, select it on the board and press `l`. You see its sign-off card and the commits that will land. Press `y` to push them to the default branch. The task closes and its worktree goes back to the pool.

Next: [The board](board.md) and [Tasks](tasks.md).
