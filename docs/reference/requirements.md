# Requirements

kelpie runs on macOS and Linux, inside herdr 0.9.0 or later.

## Tools

| Tool | Needed for | macOS | Linux |
| --- | --- | --- | --- |
| `git` | everything | `xcode-select --install` | `sudo apt install git` |
| `herdr` 0.9.0+ | everything | `brew install herdr` | `curl -fsSL https://herdr.dev/install.sh \| sh` |
| `claude` (Claude Code) | everything | `curl -fsSL https://claude.ai/install.sh \| bash` | the same |
| `jq` | the sidebar sync and the review hook | `brew install jq` | `sudo apt install jq` |
| `cargo` | building kelpie on the `main` channel or from `kelpie update --from`; without rustup, `rustc` must be at least the `rust-version` in `Cargo.toml` | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh` | the same |
| `fzf` 0.71+ | the review split's file list | `brew install fzf` | `git clone --depth 1 https://github.com/junegunn/fzf.git ~/.fzf && ~/.fzf/install` |
| `nvim` 0.10+ | the review split's diffs | `brew install neovim` | a [release](https://github.com/neovim/neovim/releases) |
| `curl` | downloading a release's prebuilt binary, and the review list reloading on change | `brew install curl` | `sudo apt install curl` |
| `tig` | the history split | `brew install tig` | `sudo apt install tig` |

Distro packages of `fzf` and `nvim` are often older than these versions.

The task sandbox (`task.sandbox`, on by default) needs Claude Code 2.1.285 or later. On Linux it also needs `bubblewrap` and `socat` (`sudo apt install bubblewrap socat`) and the seccomp filter that blocks Unix sockets (`npm install -g @anthropic-ai/sandbox-runtime`). Without them kelpie refuses to start tasks until they are installed or `task.sandbox = false`. macOS needs nothing more.

Rust is only needed where kelpie is built from source. A release tag installs a prebuilt binary on macOS (Apple silicon and Intel) and on Linux with glibc (x86_64 and arm64); see [Prebuilt binaries](../explanation/updates-and-restarts.md#prebuilt-binaries).

Plan usage on the board needs a Claude.ai plan; an API key gives no usage numbers.

A task whose profile runs Codex needs Codex CLI 0.163.0 or later (`curl -fsSL https://chatgpt.com/codex/install.sh | sh`, `brew install --cask codex` or `npm install -g @openai/codex`) and kelpie's plugin in it (`kelpie setup codex`).

## What `kelpie doctor` checks

Each line is `ok`, `warn` or `fail`. Every `warn` and `fail` carries a `fix:` line for macOS and Linux. `kelpie doctor` exits non-zero when any line fails.

| Check | Looks at |
| --- | --- |
| `git`, `herdr`, `claude`, `jq`, `fzf`, `nvim`, `tig`, `curl`, `cargo` | Present on `PATH`, at the versions above. A missing `cargo` is fine on the release channel with a downloaded binary. |
| `herdr plugin` | kelpie installed and enabled in herdr. |
| `plugin binary` | The plugin's binary is in place, and whether it was downloaded for a release tag or built from source. |
| `claude plugin` | The Claude Code plugin is installed, and the copy Claude Code runs matches the checkout. |
| `claude plugin source` | The plugin's marketplace matches the update channel. |
| `status line` | The status line for plan usage. |
| `current link` | `~/.local/share/kelpie/current` points at the plugin. |
| `kelpie on PATH` | `kelpie` on `PATH` is the `current` link's binary. |
| `updates` | The last update check, without fetching. |
| `keeper` | Whether the keeper runs, with its pid and uptime, or that `[keeper] enabled = false` turns it off. |
| `state dir` | The state directories. |
| `config`, `repo config` | config.toml and every file in the repos folder parse, with no unknown or misplaced keys and no broken links. |
| `push block` | Inside a repo: whether its task worktrees get the push block. |
| `docs` | Inside a repo: whether `docs.required` is on, and a suggestion to turn it on when `gh` says the repo is public on GitHub. Without `gh` or a GitHub remote the line is left out. |
| `setup skill` | The setup skill's plugin is installed and enabled. |
| `hub` | Whether the machine joined a kelpie hub; when it has, the hub's address and the machine's name, how many phones are paired and when the board last went out, warning when the hub does not answer, no phone is paired or the last post failed. |
| `notify` | Whether `notify.command` is set. It is never run. |
| `sandbox` | Whether tasks run in the sandbox: on and ready, on with what is missing on Linux (a `fail`, since tasks then do not start), or off with `task.sandbox = false` (a `warn` that says a task's shell can pass as you). |
| `vault` | The vault plugin: on, missing (fine, it is optional) or too old. |
| `codex`, `codex plugin`, `codex hooks` | Codex on `PATH` at 0.163.0 or later, kelpie's plugin in its cache and current, and nothing in Codex's `config.toml` that turns kelpie's hooks off (`[features] hooks = false`, or one of them turned off in `/hooks`). Only `codex` shows until a profile runs Codex or Codex is on `PATH`; a gap is a `warn` until a profile runs Codex, and a `fail` after. |
| `backlog …` | Each backlog source can be read, and an installed source not in use. |

## Repos

A repo needs a remote named `origin` with a default branch: `origin/HEAD`, else `origin/main` or `origin/master`. Without one, kelpie says to run `git remote set-head origin -a`.

The push block uses git's per-worktree config. A repo whose shared git config sets `core.worktree` or `core.bare = true` gets no push block until those are moved; `kelpie start` and `kelpie doctor` say so.
