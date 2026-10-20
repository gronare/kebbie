# Requirements

kelpie runs on macOS and Linux, inside herdr 0.9.0 or later.

## Tools

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

Plan usage on the board needs a Claude.ai plan; an API key gives no usage numbers.

## What `kelpie doctor` checks

Each line is `ok`, `warn` or `fail`. Every `warn` and `fail` carries a `fix:` line for macOS and Linux. `kelpie doctor` exits non-zero when any line fails.

| Check | Looks at |
| --- | --- |
| `git`, `herdr`, `claude`, `jq`, `fzf`, `nvim`, `tig`, `curl`, `cargo` | Present on `PATH`, at the versions above. |
| `herdr plugin` | kelpie installed and enabled in herdr. |
| `plugin binary` | The plugin's binary is built. |
| `claude plugin` | The Claude Code plugin is installed, and the copy Claude Code runs matches the checkout. |
| `claude plugin source` | The plugin's marketplace matches the update channel. |
| `status line` | The status line for plan usage. |
| `current link` | `~/.local/share/kelpie/current` points at the plugin. |
| `kelpie on PATH` | `kelpie` on `PATH` is the `current` link's binary. |
| `updates` | The last update check, without fetching. |
| `state dir` | The state directories. |
| `config`, `repo config` | config.toml and every file in the repos folder parse, with no unknown or misplaced keys and no broken links. |
| `push block` | Inside a repo: whether its task worktrees get the push block. |
| `setup skill` | The setup skill's plugin is installed and enabled. |
| `notify` | Whether `notify.command` is set. It is never run. |
| `vault` | The vault plugin: on, missing (fine, it is optional) or too old. |
| `backlog …` | Each backlog source can be read, and an installed source not in use. |

## Repos

A repo needs a remote named `origin` with a default branch: `origin/HEAD`, else `origin/main` or `origin/master`. Without one, kelpie says to run `git remote set-head origin -a`.

The push block uses git's per-worktree config. A repo whose shared git config sets `core.worktree` or `core.bare = true` gets no push block until those are moved; `kelpie start` and `kelpie doctor` say so.
